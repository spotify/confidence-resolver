//! Flag-log shipping, behind one abstraction.
//!
//! Two sinks, chosen once per isolate from the `FLAG_LOG_SINK` variable:
//!
//! * [`Sink::Queue`] (the default) — the log is published to a Cloudflare
//!   Queue shard and the queue consumer aggregates up to 100 messages before
//!   delivery. A publish that fails is dropped. See [`queue`] and [`shards`].
//! * [`Sink::Buffer`] — logs accumulate in isolate memory and are aggregated
//!   and POSTed straight to Confidence, with nothing in between. See
//!   [`buffer`].
//!
//! The trade is cost and throughput against durability.
//!
//! The queue bills per message — three operations each — so cost scales with
//! the number of log records, and each queue caps out around 5,000 messages a
//! second, so high rates need shards. The buffer bills nothing per record and
//! has no shared component to saturate: Cloudflare runs more isolates as load
//! grows and each delivers its own buffer, so throughput scales with the
//! traffic itself.
//!
//! Durability runs the other way. Both sinks ship from `wait_until`, which
//! Cloudflare does not guarantee to run, but the queue gives at-least-once
//! delivery to its consumer once a publish succeeds, and only ever has one
//! log in the air. The buffer has no durable hand-off at all: Cloudflare
//! offers no shutdown hook, so an isolate evicted while holding a buffer
//! loses it silently. Both sinks retry transient failures up to three
//! times per destination, but a batch that exhausts its attempts is dropped.
//!
//! Queue bindings are created under either sink, so switching `FLAG_LOG_SINK`
//! back to `queue` is an immediate rollback that also drains anything still
//! in flight.
mod buffer;
mod queue;
mod shards;

use confidence_resolver::{
    apply_dedup::{compute_applied_flag_dedup_hash, AppliedFlagRef, ApplyDedup},
    flag_logger,
    proto::confidence::flags::resolver::v1::WriteFlagLogsRequest,
};
use std::sync::OnceLock;
use worker::{console_log, Env, MessageBatch, Result};

/// Where this deployment ships flag logs.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub(crate) enum Sink {
    #[default]
    Queue,
    /// Accumulate in isolate memory and POST straight to Confidence, with
    /// nothing in between. See [`buffer`].
    Buffer,
}

impl Sink {
    /// Anything but an explicit `buffer` keeps the queue, so a typo degrades
    /// to the durable path instead of silently risking every log.
    fn parse(raw: Option<&str>) -> Self {
        match raw.map(str::trim) {
            Some(value) if value.eq_ignore_ascii_case("buffer") => Sink::Buffer,
            _ => Sink::Queue,
        }
    }
}

static SINK: OnceLock<Sink> = OnceLock::new();
static METRICS_KV: OnceLock<Option<worker::kv::KvStore>> = OnceLock::new();

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

    METRICS_KV.get_or_init(|| env.kv("CONFIDENCE_METRICS_KV").ok());
}

fn sink() -> Sink {
    SINK.get().copied().unwrap_or_default()
}

/// Emits the log inline where the active sink allows it, returning the log
/// back when shipping it needs async work.
///
/// `wait_until` is best effort: Cloudflare can cancel pending work when an
/// isolate is evicted or the post-response budget is exceeded. The buffer
/// sink's common path is an in-memory append, so it runs here, inside the
/// request, and returns a batch only when a flush is due. Neither sink does
/// I/O here: a network round-trip in the request path would charge the
/// caller for it.
/// Result of [`emit_inline`]: either a single log to queue, or a batch the
/// buffer decided to flush.
pub(crate) enum Emitted {
    /// Ship this single log (queue mode).
    One(Box<WriteFlagLogsRequest>),
    /// Deliver this flushed batch (buffer mode).
    Batch(Vec<WriteFlagLogsRequest>),
}

pub(crate) fn emit_inline(log: WriteFlagLogsRequest) -> Option<Emitted> {
    match sink() {
        Sink::Queue => Some(Emitted::One(Box::new(log))),
        Sink::Buffer => buffer::offer(log).map(Emitted::Batch),
    }
}

/// Ships one request's flag log. Called from `wait_until`, so it runs after
/// the response has been returned.
pub(crate) async fn send(emitted: Emitted) {
    match emitted {
        Emitted::One(log) => queue::send(*log).await,
        Emitted::Batch(logs) => buffer::deliver(logs).await,
    }
}

/// Runs after every request, from `wait_until`.
///
/// Only the buffer sink needs it: the size trigger fires in the request path,
/// but the idle and age triggers need something still running once the
/// traffic stops. A no-op for the sinks that ship each log as it arrives.
pub(crate) async fn tick() {
    if sink() == Sink::Buffer {
        buffer::tick().await;
    }
}

/// Updates the KV telemetry snapshot after a buffer delivery.
pub(super) async fn update_metrics(
    telemetry: Option<&confidence_resolver::proto::confidence::flags::resolver::v1::TelemetryData>,
    delivered: bool,
) {
    if let Some(Some(kv)) = METRICS_KV.get() {
        crate::update_kv_snapshot(
            kv,
            crate::SnapshotPipeline::FlagLogs,
            crate::request_telemetry_to_accumulate(telemetry, delivered),
            Some(delivered),
            None,
        )
        .await;
    }
}

/// Queue-consumer entry point. Stays wired under either sink so a rollback
/// drains whatever was queued before the switch.
pub(crate) async fn consume(batch: MessageBatch<String>, env: Env) -> Result<()> {
    queue::consume(batch, env).await
}

/// Attempts per destination, including the first.
///
/// Bounded deliberately rather than generous: this runs in `wait_until`, so
/// every extra attempt is more time the records exist only in this isolate,
/// and for the buffer sink it also holds an in-flight slot that other
/// flushes are waiting on. Three attempts covers a backend blip; riding out
/// a real outage is not something an in-memory sink can do.
const DELIVERY_ATTEMPTS: u32 = 3;

/// Backoff before retry *n* (1-based). Fixed rather than exponential-to-the-
/// sky for the same reason the attempt count is small.
fn retry_backoff_ms(attempt: u32) -> u64 {
    match attempt {
        1 => 250,
        _ => 1_000,
    }
}

/// Walks the configured destinations in order, stopping at the first success.
///
/// Each destination gets up to [`DELIVERY_ATTEMPTS`] tries, but only while
/// the failure looks transient — see [`crate::DeliveryError::is_retryable`].
/// A `413` or `401` moves straight on to the next destination rather than
/// failing the same way twice more.
async fn deliver(req: &WriteFlagLogsRequest) -> bool {
    let Some(client_secret) = crate::CONFIDENCE_CLIENT_SECRET.get() else {
        console_log!("flag log delivery skipped: client secret unavailable");
        return false;
    };
    let account_id = crate::CDN_STATE_REQUEST.account_id.as_str();

    for &destination in crate::LOG_DESTINATIONS.iter() {
        for attempt in 1..=DELIVERY_ATTEMPTS {
            let started_ms = js_sys::Date::now();
            let result =
                crate::deliver_flag_logs(client_secret, account_id, req, destination).await;
            let elapsed_ms = (js_sys::Date::now() - started_ms) as u64;
            match result {
                Ok(()) => {
                    // Timed per destination: a slow first destination and a
                    // slow backend look identical in an aggregate figure, and
                    // they call for completely different fixes.
                    console_log!(
                        "flag log delivered to {:?} in {}ms on attempt {} ({} assigns, {} flags)",
                        destination,
                        elapsed_ms,
                        attempt,
                        req.flag_assigned.len(),
                        req.flag_resolve_info.len()
                    );
                    return true;
                }
                Err(reason) => {
                    let retrying = reason.is_retryable() && attempt < DELIVERY_ATTEMPTS;
                    console_log!(
                        "flag log delivery to {:?} failed after {}ms (attempt {}/{}): {}{}",
                        destination,
                        elapsed_ms,
                        attempt,
                        DELIVERY_ATTEMPTS,
                        reason,
                        if retrying { "; retrying" } else { "" }
                    );
                    if !retrying {
                        break;
                    }
                    worker::Delay::from(std::time::Duration::from_millis(retry_backoff_ms(
                        attempt,
                    )))
                    .await;
                }
            }
        }
    }
    false
}

/// A dedup window bounded by entry count.
///
/// The TTL argument is inert here: `ApplyDedup` expires entries only in
/// `sweep`, and the queue consumer never sweeps — it builds a fresh map per
/// batch, so there is nothing for a TTL to expire.
///
/// Bounding by entry count instead makes the window's behaviour independent
/// of any clock: the first 100k distinct assignments are deduplicated, and
/// the caller decides what to do when it fills.
const DEDUP_MAX_ENTRIES: usize = 100_000;

fn new_dedup() -> ApplyDedup {
    // TTL unused; see above.
    ApplyDedup::new(i64::MAX, DEDUP_MAX_ENTRIES)
}

/// Encoded-protobuf budget per delivery.
///
/// Sized so the JSON body stays under [`MAX_DELIVERY_BYTES`] on the first
/// try. Measured against a heavy flag set, JSON runs several times the
/// encoded size, so this is deliberately well below the JSON cap rather than
/// close to it — overshooting costs a wasted aggregate and a failed delivery.
const PROTO_CHUNK_BYTES: usize = 768 * 1024;

/// Largest body the backend accepts, less headroom.
///
/// Measured by bisection: it takes 4 MiB and returns `413` above, which no
/// retry recovers from. Callers therefore split their records to fit rather
/// than assuming a batch is small enough: the buffer's flush budget is a
/// target, not a guarantee, and aggregation does not shrink exposures.
const MAX_DELIVERY_BYTES: usize = 3 * 1024 * 1024 + 512 * 1024;

/// Largest body observed to be accepted, from the bisection. Kept here so
/// the headroom below is checked against a measurement rather than a memory.
const MEASURED_BACKEND_LIMIT: usize = 4_193_298;

/// Checked at compile time: raising the split threshold into the backend's
/// ceiling would reintroduce the `413` that no retry can recover from.
const _: () = assert!(MAX_DELIVERY_BYTES < MEASURED_BACKEND_LIMIT);
const _: () = assert!(MEASURED_BACKEND_LIMIT - MAX_DELIVERY_BYTES >= 500_000);

/// Aggregates and delivers to every configured destination.
///
/// Splits on the encoded size *before* aggregating rather than aggregating
/// and splitting after. Aggregating first means building a merged copy and a
/// JSON body only to discover they are too big, then doing it again for each
/// half — measured, that recursion allocated tens of MB per flush and
/// trapped the isolate with `memory access out of bounds` under load.
/// `split_off` moves records instead of cloning them, so only the aggregate
/// that is actually delivered is ever allocated.
///
/// The JSON size is still checked, because `encoded_len` is a protobuf
/// measure and the body is JSON; the halving below is the backstop for a
/// batch whose expansion is worse than [`PROTO_CHUNK_BYTES`] assumes.
pub(super) fn deliver_all_within_limit(
    logs: Vec<WriteFlagLogsRequest>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool>>> {
    Box::pin(async move {
        if logs.is_empty() {
            return true;
        }

        let encoded: usize = logs.iter().map(prost::Message::encoded_len).sum();
        if encoded > PROTO_CHUNK_BYTES && logs.len() > 1 {
            let mut head = logs;
            let tail = head.split_off(head.len() / 2);
            // Both halves must be attempted even if the first fails: `&&` would
            // short-circuit and silently drop the tail.
            let a = deliver_all_within_limit(head).await;
            let b = deliver_all_within_limit(tail).await;
            return a && b;
        }

        let count = logs.len();
        let request = flag_logger::aggregate_batch(logs);
        let size = serde_json::to_string(&request).map_or(usize::MAX, |json| json.len());
        if size <= MAX_DELIVERY_BYTES {
            return deliver(&request).await;
        }
        // Between the split threshold and the measured ceiling is the
        // headroom this module reserves. Aggregation has already consumed
        // the records, so the choice is to spend that headroom or to drop a
        // batch the backend would in fact have accepted. Spend it, and say
        // so: a run of these means `PROTO_CHUNK_BYTES` is mis-calibrated for
        // this account's records.
        if size < MEASURED_BACKEND_LIMIT {
            console_log!(
                "flag log: aggregate of {} records is {} bytes, over the {} byte split \
                 threshold but within the {} byte backend limit; delivering on reserved \
                 headroom",
                count,
                size,
                MAX_DELIVERY_BYTES,
                MEASURED_BACKEND_LIMIT
            );
            return deliver(&request).await;
        }
        // Past the measured limit the backend answers 413, and the records
        // are gone into the aggregate so there is nothing left to split.
        console_log!(
            "flag log: DROPPED {} records, aggregate of {} bytes is past the {} byte \
             backend limit",
            count,
            size,
            MEASURED_BACKEND_LIMIT
        );
        // Report as a failure so buffer::deliver logs DROPPED and the KV
        // metrics count it as failed. The drop is already logged above.
        false
    })
}

/// Removes applied flags already seen in `dedup`.
///
/// The map is taken by reference so one window can span several batches,
/// which the resolve-time pass needs.
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
    fn recognises_buffer_case_insensitively_and_trimmed() {
        for raw in ["buffer", "BUFFER", "Buffer", "  buffer  ", "\tbuffer\n"] {
            assert_eq!(Sink::parse(Some(raw)), Sink::Buffer, "raw = {raw:?}");
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
        // A typo must not silently pick the sink with no durable hand-off:
        // the queue is the path that is already wired and already has a
        // consumer.
        for raw in [
            "",
            "bufer",
            "buffered",
            "memory",
            "true",
            "buffer2",
            "in buffer",
        ] {
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

    /// The window is shared across calls rather than rebuilt per call, so a
    /// duplicate spanning two batches is still caught. The queue consumer
    /// relies on this: a repeated assignment rarely lands twice inside one
    /// batch, so a fresh map per batch would miss most of them.
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
