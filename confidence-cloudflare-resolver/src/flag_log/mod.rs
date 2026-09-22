//! Flag-log shipping, behind one abstraction.
//!
//! Two sinks, chosen once per isolate from the `FLAG_LOG_SINK` variable:
//!
//! * [`Sink::Queue`] (the default) — the log is published to a Cloudflare
//!   Queue shard and the queue consumer aggregates up to 100 messages before
//!   delivery. A publish that fails is dropped. See [`queue`] and [`shards`].
//! * [`Sink::Logpush`] — the log is written to `console.log` and Cloudflare's
//!   Logpush POSTs batches of trace events back to this Worker, which
//!   aggregates and delivers them. See [`logpush`] and [`ingest`].
//!
//! The sinks differ in where the batching happens, and that is the trade. The
//! queue is billed per message — three operations each — so cost scales with
//! the number of log records. Logpush is billed per *request*, which you are
//! serving anyway, and Cloudflare batches thousands of records into each
//! push.
//!
//! Durability differs too, in both directions. The queue sink publishes from
//! `wait_until`, which Cloudflare does not guarantee to run; the Logpush sink
//! writes to the console inline, during the response, so an eviction cannot
//! take the log with it. Downstream the comparison reverses: the queue gives
//! at-least-once delivery to its consumer once a publish succeeds, whereas
//! Logpush drops a batch that the ingest route cannot deliver. Neither sink
//! retries a failed delivery.
//!
//! Queue bindings are created under either sink, so switching `FLAG_LOG_SINK`
//! back to `queue` is an immediate rollback that also drains anything still
//! in flight.
mod ingest;
mod logpush;
mod queue;
mod shards;

pub(crate) use ingest::handle as handle_ingest;

use confidence_resolver::{
    apply_dedup::{compute_applied_flag_dedup_hash, AppliedFlagRef, ApplyDedup},
    flag_logger,
    proto::confidence::flags::resolver::v1::WriteFlagLogsRequest,
};
use std::sync::OnceLock;
use worker::{console_log, Env, MessageBatch, Result};

/// Marks a `console.log` line as an encoded [`WriteFlagLogsRequest`].
///
/// A trace event carries every console line the invocation emitted, and
/// Logpush filters cannot reach inside the `Logs` array — it is typed
/// `array[object]`, which filtering does not support. The prefix is what lets
/// the ingest route keep flag logs and ignore diagnostics, panics, and
/// anything a future change starts logging on the same request.
const FLAG_LOG_PREFIX: &str = "FLAGLOG ";

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

/// Account id the deployer reads out of the resolver state and passes as a
/// variable, so delivery does not need the state itself.
static ACCOUNT_ID: OnceLock<String> = OnceLock::new();

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

    if ACCOUNT_ID.get().is_none() {
        if let Ok(account) = env.var("CONFIDENCE_ACCOUNT_ID").map(|v| v.to_string()) {
            let _ = ACCOUNT_ID.set(account);
        }
    }
}

fn sink() -> Sink {
    SINK.get().copied().unwrap_or_default()
}

/// Emits the log inline where the active sink allows it, returning the log
/// back when shipping it needs async work.
///
/// `wait_until` is best effort: Cloudflare can cancel pending work when an
/// isolate is evicted or the post-response budget is exceeded. The Logpush
/// sink's common path is a local console write, so it runs here, inside the
/// request — which is what makes its durability claim true rather than
/// aspirational. The queue sink cannot do this: publishing is a network
/// round-trip, and awaiting it in the request path would charge the caller
/// for it.
pub(crate) fn emit_inline(log: WriteFlagLogsRequest) -> Option<WriteFlagLogsRequest> {
    match sink() {
        Sink::Queue => Some(log),
        Sink::Logpush => logpush::emit_inline(log),
    }
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

/// Delivers to one named destination.
///
/// The destination comes from the log line rather than from state, and the
/// account id from the environment, so this path needs neither. That is what
/// lets the ingest handler run without the resolver state loaded.
async fn deliver_to(destination: i32, req: &WriteFlagLogsRequest) -> bool {
    let Some(client_secret) = crate::CONFIDENCE_CLIENT_SECRET.get() else {
        console_log!("flag log delivery skipped: client secret unavailable");
        return false;
    };
    let destination = confidence_resolver::LogDestination::from(destination);
    let account_id = account_id();

    let started_ms = js_sys::Date::now();
    match crate::deliver_flag_logs(client_secret, &account_id, req, destination).await {
        Ok(()) => {
            console_log!(
                "flag log delivered to {:?} in {}ms ({} assigns, {} flags)",
                destination,
                (js_sys::Date::now() - started_ms) as u64,
                req.flag_assigned.len(),
                req.flag_resolve_info.len()
            );
            true
        }
        Err(reason) => {
            console_log!(
                "flag log delivery to {:?} failed after {}ms: {}",
                destination,
                (js_sys::Date::now() - started_ms) as u64,
                reason
            );
            false
        }
    }
}

/// Account id, from the environment where the deployer put it, falling back
/// to the embedded state for the queue sink which already has it loaded.
fn account_id() -> String {
    ACCOUNT_ID
        .get()
        .cloned()
        .unwrap_or_else(|| crate::CDN_STATE_REQUEST.account_id.clone())
}

/// Walks the configured destinations in order, stopping at the first success.
///
/// Used by the queue consumer, so the queue path keeps main's behaviour
/// exactly: try each destination in order and stop at the first success.
async fn deliver(req: &WriteFlagLogsRequest) -> bool {
    let Some(client_secret) = crate::CONFIDENCE_CLIENT_SECRET.get() else {
        console_log!("flag log delivery skipped: client secret unavailable");
        return false;
    };
    let account_id = crate::CDN_STATE_REQUEST.account_id.as_str();

    for &destination in crate::LOG_DESTINATIONS.iter() {
        let started_ms = js_sys::Date::now();
        let result = crate::deliver_flag_logs(client_secret, account_id, req, destination).await;
        let elapsed_ms = (js_sys::Date::now() - started_ms) as u64;
        match result {
            Ok(()) => {
                // Timed per destination: a slow first destination and a slow
                // backend look identical in an aggregate figure, and they
                // call for completely different fixes.
                console_log!(
                    "flag log delivered to {:?} in {}ms ({} assigns, {} flags)",
                    destination,
                    elapsed_ms,
                    req.flag_assigned.len(),
                    req.flag_resolve_info.len()
                );
                return true;
            }
            Err(reason) => console_log!(
                "flag log delivery to {:?} failed after {}ms: {}",
                destination,
                elapsed_ms,
                reason
            ),
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

/// Largest body the backend accepts, less headroom.
///
/// Measured by bisection: it takes 4 MiB and returns `413` above, which no
/// retry recovers from — a rejected object would be redelivered, rejected
/// again, and eventually dead-lettered. The consumer therefore splits its
/// records to fit rather than trusting the object to be small enough.
///
/// Splitting here rather than sizing the Logpush object is deliberate:
/// `max_upload_records` has a floor of 1,000 and `max_upload_bytes` one of
/// several MB, so object granularity is not ours to choose. Owning the
/// delivery size locally makes the consumer correct for any object it is
/// handed.
const MAX_DELIVERY_BYTES: usize = 3 * 1024 * 1024 + 512 * 1024;

/// Largest body observed to be accepted, from the bisection. Kept here so
/// the headroom below is checked against a measurement rather than a memory.
const MEASURED_BACKEND_LIMIT: usize = 4_193_298;

/// Checked at compile time: raising the split threshold into the backend's
/// ceiling would reintroduce the `413` that no retry can recover from.
const _: () = assert!(MAX_DELIVERY_BYTES < MEASURED_BACKEND_LIMIT);
const _: () = assert!(MEASURED_BACKEND_LIMIT - MAX_DELIVERY_BYTES >= 500_000);

/// Aggregates and delivers, splitting until each body fits the backend.
///
/// Recursive halving rather than a size estimate: the serialized size of a
/// batch is not a simple function of its record count — flag counts and
/// context sizes vary — and being wrong means a `413` that retrying cannot
/// fix. Measuring the actual body and splitting when it is too big is exact.
///
/// In the common case the whole object fits and this is one aggregate and one
/// delivery, the same as before.
pub(super) fn deliver_within_limit(
    destination: i32,
    logs: Vec<WriteFlagLogsRequest>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool>>> {
    Box::pin(async move {
        if logs.is_empty() {
            return true;
        }
        let request = flag_logger::aggregate_batch(logs.clone());
        let size = serde_json::to_string(&request).map_or(usize::MAX, |json| json.len());

        if size <= MAX_DELIVERY_BYTES {
            return deliver_to(destination, &request).await;
        }
        if logs.len() == 1 {
            // A single record over the limit cannot be split any further.
            console_log!(
                "flag log objects: single record of {} bytes exceeds the {} byte limit; dropping",
                size,
                MAX_DELIVERY_BYTES
            );
            return true;
        }

        console_log!(
            "flag log objects: aggregate of {} records is {} bytes, splitting",
            logs.len(),
            size
        );
        let mut halves = logs;
        let tail = halves.split_off(halves.len() / 2);
        // Sequential, not concurrent: a split means the payload is already
        // near the limit, so holding two of them decoded at once is exactly
        // the memory spike worth avoiding.
        deliver_within_limit(destination, halves).await
            && deliver_within_limit(destination, tail).await
    })
}

/// Removes applied flags already seen in `dedup`.
///
/// The map is taken by reference so one window can span several batches,
/// which the resolve-time pass needs; the ingest route builds a fresh map per
/// Logpush batch because each batch is self-contained.
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
