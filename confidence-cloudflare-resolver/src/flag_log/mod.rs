//! Flag-log shipping, behind one abstraction.
//!
//! Two sinks, chosen once per isolate from the `FLAG_LOG_SINK` variable:
//!
//! * [`Sink::Queue`] (the default) — the log is published to a Cloudflare
//!   Queue shard and the queue consumer aggregates up to 100 messages before
//!   delivery. A publish that fails is dropped. See [`queue`] and [`shards`].
//! * [`Sink::Logpush`] — the log is written to `console.log`, Cloudflare's
//!   Logpush captures it into R2, and a cron-triggered pass aggregates,
//!   delivers, and deletes. See [`logpush`] and [`aggregator`].
//!
//! The sinks differ in where the batching happens, and that is the trade. The
//! queue is billed per message — three operations each — so cost scales with
//! the number of log records. Logpush is billed per *request*, which you are
//! serving anyway, and Cloudflare batches thousands of records into each R2
//! object for the price of one write.
//!
//! Durability differs too, in both directions. Both sinks emit from
//! `wait_until`, so a dropped `wait_until` loses the log either way; Logpush
//! only narrows that window, by writing to the console instead of making a
//! network round-trip to the queue. From there they diverge: the queue gives
//! at-least-once delivery to its consumer once a publish succeeds, whereas a
//! failed publish is unrecoverable; Logpush drops a batch permanently after
//! roughly five minutes of failures, but once an object lands in R2 the
//! aggregator owns the retry.
//!
//! Queue bindings are created under either sink, so switching `FLAG_LOG_SINK`
//! back to `queue` is an immediate rollback that also drains anything still
//! in flight.
mod aggregator;
mod logpush;
mod queue;
mod shards;

pub(crate) use aggregator::run as run_aggregator;

use confidence_resolver::{
    apply_dedup::{compute_applied_flag_dedup_hash, AppliedFlagRef, ApplyDedup},
    proto::confidence::flags::resolver::v1::WriteFlagLogsRequest,
};
use std::{cell::RefCell, sync::OnceLock};
use worker::{console_log, Bucket, Env, MessageBatch, Result};

/// Marks a `console.log` line as an encoded [`WriteFlagLogsRequest`].
///
/// A trace event carries every console line the invocation emitted, and
/// Logpush filters cannot reach inside the `Logs` array — it is typed
/// `array[object]`, which filtering does not support. The prefix is what lets
/// the aggregator keep flag logs and ignore diagnostics, panics, and anything
/// a future change starts logging on the same request.
const FLAG_LOG_PREFIX: &str = "FLAGLOG ";

/// R2 binding the deployer adds when `FLAG_LOG_SINK=logpush`. Logpush writes
/// into it; the aggregator drains it; oversized logs are written to it
/// directly.
const BUCKET_BINDING: &str = "FLAG_LOGS_R2";

/// Where this deployment ships flag logs.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub(crate) enum Sink {
    #[default]
    Queue,
    Logpush,
}

impl Sink {
    /// Anything but an explicit `logpush` keeps the queue, so a typo degrades
    /// to the established path instead of silently dropping every log.
    fn parse(raw: Option<&str>) -> Self {
        match raw.map(str::trim) {
            Some(value) if value.eq_ignore_ascii_case("logpush") => Sink::Logpush,
            _ => Sink::Queue,
        }
    }
}

static SINK: OnceLock<Sink> = OnceLock::new();

thread_local! {
    /// `Bucket` wraps a `JsValue`, so it is neither `Send` nor `Sync` and
    /// cannot live in a static the way the queue bindings do.
    ///
    /// The outer `Option` records whether the lookup has been attempted and
    /// the inner one whether it succeeded, so a missing binding is reported
    /// once per isolate rather than once per request.
    static BUCKET: RefCell<Option<Option<Bucket>>> = const { RefCell::new(None) };
}

/// Resolves the sink and binds whatever it needs. Call once per entry point,
/// before [`send`].
pub(crate) fn init(env: &Env) {
    let sink = *SINK.get_or_init(|| {
        let raw = env.var("FLAG_LOG_SINK").map(|var| var.to_string()).ok();
        Sink::parse(raw.as_deref())
    });

    if sink == Sink::Queue {
        queue::init(env);
    }

    // The bucket is bound whenever it exists, under *either* sink. A switch
    // back to `queue` has to keep draining whatever Logpush already wrote —
    // Logpush lags by about a minute and keeps writing after the switch — so
    // gating this on the active sink would strand those objects silently.
    BUCKET.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() {
            return;
        }
        match env.bucket(BUCKET_BINDING) {
            Ok(bucket) => *slot = Some(Some(bucket)),
            Err(e) => {
                // Absent is the norm for a queue-only deployment, so this is
                // only worth reporting when Logpush is the active sink: there
                // it means a half-provisioned deploy, where oversized logs
                // lose their overflow path and nothing drains the bucket.
                if sink == Sink::Logpush {
                    console_log!(
                        "{} binding is missing; oversized flag logs will fall back to \
                         direct delivery and R2 aggregation is disabled: {:?}",
                        BUCKET_BINDING,
                        e
                    );
                }
                *slot = Some(None);
            }
        }
    });
}

fn sink() -> Sink {
    SINK.get().copied().unwrap_or_default()
}

/// Cloning a `Bucket` clones the underlying `JsValue` handle, so this is
/// cheap and avoids handing out a borrow of thread-local state across an
/// await point.
fn bucket() -> Option<Bucket> {
    BUCKET.with(|slot| slot.borrow().clone().flatten())
}

/// Ships one request's flag log. Called from `wait_until`, so it runs after
/// the response has been returned.
pub(crate) async fn send(log: WriteFlagLogsRequest) {
    match sink() {
        Sink::Queue => queue::send(log).await,
        Sink::Logpush => logpush::send(log).await,
    }
}

/// Queue-consumer entry point. Stays wired under either sink so a rollback
/// drains whatever was queued before the switch.
pub(crate) async fn consume(batch: MessageBatch<String>, env: Env) -> Result<()> {
    queue::consume(batch, env).await
}

/// Walks the configured destinations in order, stopping at the first success.
///
/// Shared by the queue consumer, the R2 aggregator, and the oversized-log
/// fallback, so they cannot drift apart on which destinations they try or how
/// they report a failure.
async fn deliver(req: &WriteFlagLogsRequest) -> bool {
    let Some(client_secret) = crate::CONFIDENCE_CLIENT_SECRET.get() else {
        console_log!("flag log delivery skipped: client secret unavailable");
        return false;
    };
    let account_id = crate::CDN_STATE_REQUEST.account_id.as_str();

    for &destination in crate::LOG_DESTINATIONS.iter() {
        match crate::deliver_flag_logs(client_secret, account_id, req, destination).await {
            Ok(()) => return true,
            Err(reason) => {
                console_log!("flag log delivery to {:?} failed: {}", destination, reason)
            }
        }
    }
    false
}

/// A dedup window bounded by entry count.
///
/// The TTL argument is inert here: `ApplyDedup` expires entries only in
/// `sweep`, and neither sink sweeps. The queue consumer builds a fresh map
/// per batch, and the aggregator has no meaningful clock to sweep against —
/// it reads objects oldest-first while Logpush lags behind, so aggregation
/// wall-clock bears no fixed relation to when an apply actually happened.
/// Expiring by that measure would drop entries arbitrarily.
///
/// Bounding by entry count instead makes the window's behaviour independent
/// of any clock: the first 100k distinct assignments are deduplicated, and
/// the caller decides what to do when it fills.
const DEDUP_MAX_ENTRIES: usize = 100_000;

fn new_dedup() -> ApplyDedup {
    // TTL unused; see above.
    ApplyDedup::new(i64::MAX, DEDUP_MAX_ENTRIES)
}

/// Removes applied flags already seen in `dedup`.
///
/// The map is taken by reference so one window can span several batches. The
/// R2 aggregator needs that: it folds many objects into a single delivery and
/// keeps the map alive between passes, so a fresh map per object would miss
/// every duplicate spanning two objects — which is most of them, since an
/// object holds only about a second of traffic.
fn dedup_flag_applies(dedup: &mut ApplyDedup, logs: &mut [WriteFlagLogsRequest], now_seconds: i64) {
    for log in logs.iter_mut() {
        for assignment in &mut log.flag_assigned {
            assignment.flags.retain(|applied| {
                let hash = compute_applied_flag_dedup_hash(&AppliedFlagRef::from(applied));
                dedup.check_hash(hash, now_seconds)
            });
        }
        log.flag_assigned
            .retain(|assignment| !assignment.flags.is_empty());
    }
}

/// Deduplicates one self-contained batch. Different isolates may each log the
/// same user+flag assignment within a batch window; this removes the
/// duplicates before the network request.
fn dedup_batch_flag_applies(logs: &mut [WriteFlagLogsRequest], now_seconds: i64) {
    dedup_flag_applies(&mut new_dedup(), logs, now_seconds);
}

/// Whether the apply-event dedup pass is enabled. Defaults on.
fn dedup_enabled(env: &Env) -> bool {
    env.var("ENABLE_APPLY_DEDUP")
        .map(|var| !var.to_string().trim().eq_ignore_ascii_case("false"))
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_queue_when_unset() {
        assert_eq!(Sink::parse(None), Sink::Queue);
    }

    #[test]
    fn recognises_logpush_case_insensitively_and_trimmed() {
        for raw in [
            "logpush",
            "LOGPUSH",
            "LogPush",
            "  logpush  ",
            "\tlogpush\n",
        ] {
            assert_eq!(Sink::parse(Some(raw)), Sink::Logpush, "raw = {raw:?}");
        }
    }

    #[test]
    fn explicit_queue_selects_queue() {
        for raw in ["queue", "QUEUE", " queue "] {
            assert_eq!(Sink::parse(Some(raw)), Sink::Queue, "raw = {raw:?}");
        }
    }

    #[test]
    fn unrecognised_value_degrades_to_queue() {
        // A typo must not silently disable logging: the queue is the path
        // that is already wired and already has a consumer.
        for raw in ["", "logpsuh", "r2", "true", "logpush2", "log push"] {
            assert_eq!(Sink::parse(Some(raw)), Sink::Queue, "raw = {raw:?}");
        }
    }
}

#[cfg(test)]
mod dedup_batch_tests {
    use super::*;
    use confidence_resolver::proto::confidence::flags::resolver::v1::events::flag_assigned::{
        applied_flag::Assignment, AppliedFlag, AssignmentInfo, DefaultAssignment,
    };
    use confidence_resolver::proto::confidence::flags::resolver::v1::events::FlagAssigned;

    fn applied(flag: &str, user: &str, variant: &str) -> AppliedFlag {
        AppliedFlag {
            flag: flag.to_string(),
            targeting_key: user.to_string(),
            assignment: Some(Assignment::AssignmentInfo(AssignmentInfo {
                variant: variant.to_string(),
                segment: String::new(),
            })),
            ..Default::default()
        }
    }

    fn assigned_event(resolve_id: &str, flags: Vec<AppliedFlag>) -> FlagAssigned {
        FlagAssigned {
            resolve_id: resolve_id.to_string(),
            client_info: None,
            flags,
        }
    }

    fn log_with_assigns(assigns: Vec<FlagAssigned>) -> WriteFlagLogsRequest {
        WriteFlagLogsRequest {
            flag_assigned: assigns,
            ..Default::default()
        }
    }

    fn same_assignment(resolve_id: &str) -> Vec<WriteFlagLogsRequest> {
        vec![log_with_assigns(vec![assigned_event(
            resolve_id,
            vec![applied("flags/a", "user-1", "on")],
        )])]
    }

    /// The R2 aggregator folds many objects into a single delivery, so the
    /// dedup window has to span them. An object holds roughly a second of
    /// traffic against a 120-second window, so most duplicates arrive in
    /// *different* objects — sharing one map is what catches them.
    #[test]
    fn one_window_deduplicates_across_separate_batches() {
        let mut first = same_assignment("resolve-1");
        let mut second = same_assignment("resolve-2");

        let mut dedup = new_dedup();
        dedup_flag_applies(&mut dedup, &mut first, 1000);
        dedup_flag_applies(&mut dedup, &mut second, 1000);

        assert_eq!(first[0].flag_assigned.len(), 1, "first occurrence is kept");
        assert!(
            second[0].flag_assigned.is_empty(),
            "a repeat in a later batch must be dropped"
        );
    }

    /// Documents the bug this guards against: calling the single-batch helper
    /// once per object builds a fresh window each time and catches nothing
    /// across them.
    #[test]
    fn separate_windows_do_not_deduplicate_across_batches() {
        let mut first = same_assignment("resolve-1");
        let mut second = same_assignment("resolve-2");

        dedup_batch_flag_applies(&mut first, 1000);
        dedup_batch_flag_applies(&mut second, 1000);

        assert_eq!(first[0].flag_assigned.len(), 1);
        assert_eq!(
            second[0].flag_assigned.len(),
            1,
            "a per-batch window cannot see the earlier occurrence"
        );
    }

    #[test]
    fn no_duplicates_all_preserved() {
        let mut logs = vec![
            log_with_assigns(vec![assigned_event(
                "r1",
                vec![
                    applied("flags/a", "user-1", "on"),
                    applied("flags/b", "user-1", "off"),
                ],
            )]),
            log_with_assigns(vec![assigned_event(
                "r2",
                vec![applied("flags/c", "user-2", "on")],
            )]),
        ];

        dedup_batch_flag_applies(&mut logs, 1000);

        assert_eq!(logs[0].flag_assigned[0].flags.len(), 2);
        assert_eq!(logs[1].flag_assigned[0].flags.len(), 1);
    }

    #[test]
    fn exact_duplicate_across_messages_removed() {
        let mut logs = vec![
            log_with_assigns(vec![assigned_event(
                "r1",
                vec![applied("flags/a", "user-1", "on")],
            )]),
            log_with_assigns(vec![assigned_event(
                "r2",
                vec![applied("flags/a", "user-1", "on")],
            )]),
        ];

        dedup_batch_flag_applies(&mut logs, 1000);

        assert_eq!(logs[0].flag_assigned.len(), 1);
        assert_eq!(logs[0].flag_assigned[0].flags.len(), 1);
        assert!(logs[1].flag_assigned.is_empty());
    }

    #[test]
    fn duplicate_within_same_flag_assigned_removed() {
        let mut logs = vec![log_with_assigns(vec![assigned_event(
            "r1",
            vec![
                applied("flags/a", "user-1", "on"),
                applied("flags/a", "user-1", "on"),
            ],
        )])];

        dedup_batch_flag_applies(&mut logs, 1000);

        assert_eq!(logs[0].flag_assigned[0].flags.len(), 1);
    }

    #[test]
    fn partial_dedup_keeps_unique_flags() {
        let mut logs = vec![
            log_with_assigns(vec![assigned_event(
                "r1",
                vec![applied("flags/a", "user-1", "on")],
            )]),
            log_with_assigns(vec![assigned_event(
                "r2",
                vec![
                    applied("flags/a", "user-1", "on"),
                    applied("flags/b", "user-1", "off"),
                ],
            )]),
        ];

        dedup_batch_flag_applies(&mut logs, 1000);

        assert_eq!(logs[0].flag_assigned[0].flags.len(), 1);
        assert_eq!(logs[1].flag_assigned[0].flags.len(), 1);
        assert_eq!(logs[1].flag_assigned[0].flags[0].flag, "flags/b");
    }

    #[test]
    fn same_flag_different_users_not_deduped() {
        let mut logs = vec![log_with_assigns(vec![assigned_event(
            "r1",
            vec![
                applied("flags/a", "user-1", "on"),
                applied("flags/a", "user-2", "on"),
            ],
        )])];

        dedup_batch_flag_applies(&mut logs, 1000);

        assert_eq!(logs[0].flag_assigned[0].flags.len(), 2);
    }

    #[test]
    fn same_flag_same_user_different_variant_not_deduped() {
        let mut logs = vec![log_with_assigns(vec![assigned_event(
            "r1",
            vec![
                applied("flags/a", "user-1", "on"),
                applied("flags/a", "user-1", "off"),
            ],
        )])];

        dedup_batch_flag_applies(&mut logs, 1000);

        assert_eq!(logs[0].flag_assigned[0].flags.len(), 2);
    }

    #[test]
    fn empty_batch_is_noop() {
        let mut logs: Vec<WriteFlagLogsRequest> = vec![];
        dedup_batch_flag_applies(&mut logs, 1000);
        assert!(logs.is_empty());
    }

    #[test]
    fn logs_without_flag_assigned_unchanged() {
        use confidence_resolver::proto::confidence::flags::admin::v1::FlagResolveInfo;

        let mut logs = vec![WriteFlagLogsRequest {
            flag_resolve_info: vec![FlagResolveInfo {
                flag: "flags/a".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        }];

        dedup_batch_flag_applies(&mut logs, 1000);

        assert_eq!(logs[0].flag_resolve_info.len(), 1);
        assert!(logs[0].flag_assigned.is_empty());
    }

    #[test]
    fn default_assignment_deduped_correctly() {
        let da = AppliedFlag {
            flag: "flags/archived".to_string(),
            targeting_key: "user-1".to_string(),
            assignment: Some(Assignment::DefaultAssignment(DefaultAssignment {
                reason: 3, // FLAG_ARCHIVED
            })),
            ..Default::default()
        };

        let mut logs = vec![
            log_with_assigns(vec![assigned_event("r1", vec![da.clone()])]),
            log_with_assigns(vec![assigned_event("r2", vec![da])]),
        ];

        dedup_batch_flag_applies(&mut logs, 1000);

        assert_eq!(logs[0].flag_assigned[0].flags.len(), 1);
        assert!(logs[1].flag_assigned.is_empty());
    }

    #[test]
    fn flag_resolve_info_untouched_by_dedup() {
        use confidence_resolver::proto::confidence::flags::admin::v1::{
            flag_resolve_info::VariantResolveInfo, FlagResolveInfo,
        };

        let mut logs = vec![WriteFlagLogsRequest {
            flag_assigned: vec![
                assigned_event("r1", vec![applied("flags/a", "user-1", "on")]),
                assigned_event("r2", vec![applied("flags/a", "user-1", "on")]),
            ],
            flag_resolve_info: vec![FlagResolveInfo {
                flag: "flags/a".to_string(),
                variant_resolve_info: vec![VariantResolveInfo {
                    variant: "on".to_string(),
                    count: 42,
                }],
                ..Default::default()
            }],
            ..Default::default()
        }];

        dedup_batch_flag_applies(&mut logs, 1000);

        assert_eq!(logs[0].flag_resolve_info.len(), 1);
        assert_eq!(
            logs[0].flag_resolve_info[0].variant_resolve_info[0].count,
            42
        );
        assert_eq!(logs[0].flag_assigned[0].flags.len(), 1);
    }

    #[test]
    fn telemetry_data_preserved_even_when_all_assigns_deduped() {
        use confidence_resolver::proto::confidence::flags::resolver::v1::TelemetryData;

        let mut logs = vec![
            log_with_assigns(vec![assigned_event(
                "r1",
                vec![applied("flags/a", "user-1", "on")],
            )]),
            WriteFlagLogsRequest {
                flag_assigned: vec![assigned_event(
                    "r2",
                    vec![applied("flags/a", "user-1", "on")],
                )],
                telemetry_data: Some(TelemetryData {
                    memory_bytes: 4096,
                    ..Default::default()
                }),
                ..Default::default()
            },
        ];

        dedup_batch_flag_applies(&mut logs, 1000);

        assert!(logs[1].flag_assigned.is_empty());
        assert_eq!(logs[1].telemetry_data.as_ref().unwrap().memory_bytes, 4096);
    }

    #[test]
    fn many_duplicates_across_many_messages() {
        let mut logs: Vec<WriteFlagLogsRequest> = (0..50)
            .map(|i| {
                log_with_assigns(vec![assigned_event(
                    &format!("r{}", i),
                    vec![applied("flags/a", "user-1", "on")],
                )])
            })
            .collect();

        dedup_batch_flag_applies(&mut logs, 1000);

        let total_flags: usize = logs
            .iter()
            .flat_map(|l| &l.flag_assigned)
            .map(|fa| fa.flags.len())
            .sum();
        assert_eq!(
            total_flags, 1,
            "50 identical applies should yield 1 survivor"
        );
    }
}
