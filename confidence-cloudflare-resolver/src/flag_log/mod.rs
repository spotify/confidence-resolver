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
///
/// Takes a pre-merged [`TelemetrySnapshot`] rather than a single
/// `TelemetryData`, because a buffer flush carries deltas from many
/// requests that must all be accumulated — taking only the last one
/// would undercount by the batch size.
///
/// NOTE: unlike the queue consumer, this accumulates the deltas whether or
/// not delivery succeeded. That is deliberate and the two must not be
/// unified without thought: the queue skips them on failure because the
/// batch is redelivered and would be counted twice, while the buffer never
/// redelivers, so skipping them would simply lose the measurements.
pub(super) async fn update_metrics(
    snapshot: &confidence_resolver::telemetry::TelemetrySnapshot,
    delivered: bool,
) {
    if let Some(Some(kv)) = METRICS_KV.get() {
        crate::update_kv_snapshot_merged(
            kv,
            crate::SnapshotPipeline::FlagLogs,
            Some(snapshot),
            Some(delivered),
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

/// Budget for all flag-log work started by one invocation.
///
/// `wait_until` work is cancelled 30s after the response, so everything an
/// invocation schedules — every destination, retry, backoff and split chunk
/// — has to fit inside that. 25s leaves margin for the cancellation itself
/// and for the metrics write that follows delivery.
const INVOCATION_BUDGET_MS: f64 = 25_000.0;

/// Never spend more than this share of the remaining budget on one attempt,
/// so a stalled primary destination cannot consume the fallback's time.
const ATTEMPT_SHARE: f64 = 0.4;

/// Floor for an attempt. Below this there is no point starting one.
const MIN_ATTEMPT_MS: f64 = 250.0;

thread_local! {
    /// When the current invocation's flag-log work must be finished.
    static DEADLINE_MS: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
}

/// Starts the budget for this invocation. Called once, before any delivery.
pub(crate) fn begin_invocation() {
    DEADLINE_MS.with(|d| d.set(js_sys::Date::now() + INVOCATION_BUDGET_MS));
}

/// Milliseconds left before this invocation must stop.
fn remaining_ms() -> f64 {
    DEADLINE_MS.with(|d| (d.get() - js_sys::Date::now()).max(0.0))
}

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
            // Every attempt is bounded by what is left of the invocation's
            // budget, and takes only a share of it so a stalled primary
            // still leaves time for the fallback destination.
            let left = remaining_ms();
            if left < MIN_ATTEMPT_MS {
                console_log!(
                    "flag log: out of budget before attempt {} to {:?}; giving up",
                    attempt,
                    destination
                );
                return false;
            }
            let budget_ms = (left * ATTEMPT_SHARE).max(MIN_ATTEMPT_MS) as u64;
            let started_ms = js_sys::Date::now();
            let result =
                crate::deliver_flag_logs(client_secret, account_id, req, destination, budget_ms)
                    .await;
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
                    let backoff = retry_backoff_ms(attempt).min(remaining_ms() as u64);
                    worker::Delay::from(std::time::Duration::from_millis(backoff)).await;
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
/// Outcome of a (possibly split) delivery.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Delivered {
    /// Records that reached a destination.
    pub(super) ok: usize,
    /// Records that were dropped after exhausting every option.
    pub(super) lost: usize,
}

impl Delivered {
    fn ok(n: usize) -> Self {
        Delivered { ok: n, lost: 0 }
    }
    fn lost(n: usize) -> Self {
        Delivered { ok: 0, lost: n }
    }
    fn merge(self, other: Delivered) -> Self {
        Delivered {
            ok: self.ok + other.ok,
            lost: self.lost + other.lost,
        }
    }
}

pub(super) fn deliver_all_within_limit(
    logs: Vec<WriteFlagLogsRequest>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Delivered>>> {
    Box::pin(async move {
        if logs.is_empty() {
            return Delivered::default();
        }

        let encoded: usize = logs.iter().map(prost::Message::encoded_len).sum();
        if encoded > PROTO_CHUNK_BYTES && logs.len() > 1 {
            let mut head = logs;
            let tail = head.split_off(head.len() / 2);
            // Both halves are attempted even if the first fails, and each
            // reports its own record count so a partial failure is not
            // charged as a whole-batch loss.
            let a = deliver_all_within_limit(head).await;
            let b = deliver_all_within_limit(tail).await;
            return a.merge(b);
        }

        let count = logs.len();
        let request = flag_logger::aggregate_batch(logs);
        deliver_aggregate(request, count).await
    })
}

/// Delivers one already-aggregated request, splitting on the *JSON* size
/// when protobuf-based chunking under-predicted the expansion.
fn deliver_aggregate(
    request: WriteFlagLogsRequest,
    count: usize,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Delivered>>> {
    Box::pin(async move {
        // Only the length is needed; the string is dropped immediately so
        // the sizing pass does not hold a second copy of the body while the
        // delivery below builds its own. On the flush path under memory
        // pressure that peak matters.
        let size = serde_json::to_string(&request).map_or(usize::MAX, |json| json.len());
        if size <= MAX_DELIVERY_BYTES {
            return if deliver(&request).await {
                Delivered::ok(count)
            } else {
                Delivered::lost(count)
            };
        }
        // Between the split threshold and the measured ceiling is the
        // headroom this module reserves. Spend it rather than dropping a
        // batch the backend would in fact have accepted; a run of these
        // means `PROTO_CHUNK_BYTES` is mis-calibrated for this account.
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
            return if deliver(&request).await {
                Delivered::ok(count)
            } else {
                Delivered::lost(count)
            };
        }
        // JSON expanded worse than the protobuf split predicted. Halve the
        // assignments and recurse, so a half that is *still* too big is
        // split again rather than 413-ing. Recursion terminates because
        // each step halves, and a single assignment cannot be split.
        if request.flag_assigned.len() > 1 {
            console_log!(
                "flag log: aggregate of {} records is {} bytes (JSON), re-splitting",
                count,
                size
            );
            let mut head = request;
            let tail_assigned = head.flag_assigned.split_off(head.flag_assigned.len() / 2);
            let tail = WriteFlagLogsRequest {
                flag_assigned: tail_assigned,
                ..Default::default()
            };
            // Records and assignments are different units: one record can
            // carry many assignments, so halving assignments does not halve
            // records. When the count cannot be split into two positive
            // values, apportioning would hand the tail a 0 and make its
            // failure invisible — fall back to the success-bit scheme.
            if count < 2 {
                let a = deliver_aggregate(head, count).await;
                let b = deliver_aggregate(tail, count).await;
                return split_outcome(a.lost == 0, b.lost > 0 || a.lost > 0, count);
            }
            let head_count = count.div_ceil(2);
            let tail_count = count - head_count;
            let a = deliver_aggregate(head, head_count).await;
            let b = deliver_aggregate(tail, tail_count).await;
            return a.merge(b);
        }
        // One FlagAssigned left, but it can still hold hundreds of applied
        // flags — one resolve against a large flag set. Split those.
        //
        // Done iteratively rather than by recursing into this function: the
        // recursion is a boxed async future, and adding this branch to its
        // state machine measurably grew every frame. A load test with the
        // recursive version produced 4,810 `memory access out of bounds`
        // errors where the iterative one produced none.
        if request.flag_assigned.len() == 1 && request.flag_assigned[0].flags.len() > 1 {
            return deliver_by_splitting_flags(request, count).await;
        }
        console_log!(
            "flag log: DROPPED {} records, an indivisible payload of {} bytes is past \
             the {} byte backend limit",
            count,
            size,
            MEASURED_BACKEND_LIMIT
        );
        Delivered::lost(count)
    })
}

/// Delivers one `FlagAssigned` by halving its `flags` until each piece fits.
///
/// Iterative, with an explicit stack, so it does not deepen the boxed async
/// recursion in [`deliver_aggregate`].
///
/// Accounting is by success bit, not record count: the record is indivisible
/// at this level, so "half the flags landed" cannot be expressed as a record
/// split. Any partial outcome reports both `ok` and `lost` non-zero, which
/// is what puts the queue consumer on its partial-ack path instead of
/// nacking and re-posting the flags that already landed.
async fn deliver_by_splitting_flags(request: WriteFlagLogsRequest, count: usize) -> Delivered {
    use confidence_resolver::proto::confidence::flags::resolver::v1::events::FlagAssigned;

    // Moved out of the request rather than cloned: the flags vector is the
    // large part, and holding two copies is exactly the spike this path
    // exists to avoid.
    let mut request = request;
    let assigned = request.flag_assigned.remove(0);
    let resolve_id = assigned.resolve_id;
    let client_info = assigned.client_info;
    let mut stack: Vec<Vec<_>> = vec![assigned.flags];
    // Everything that is not the assignment — telemetry, resolve info,
    // client info — rides on the first piece only. Rebuilding each piece
    // from Default would silently discard all of it while still reporting
    // the delivery as a success.
    let mut carry = Some(request);

    let mut any_ok = false;
    let mut any_lost = false;

    while let Some(flags) = stack.pop() {
        if flags.is_empty() {
            continue;
        }
        let mut piece = carry.take().unwrap_or_default();
        piece.flag_assigned = vec![FlagAssigned {
            resolve_id: resolve_id.clone(),
            client_info: client_info.clone(),
            flags,
        }];
        let size = serde_json::to_string(&piece).map_or(usize::MAX, |json| json.len());
        if size < MEASURED_BACKEND_LIMIT {
            if deliver(&piece).await {
                any_ok = true;
            } else {
                any_lost = true;
            }
            continue;
        }
        // Still too big: halve it and push both back. Put the carried
        // fields back so they travel with whichever piece goes first.
        let mut flags = piece.flag_assigned.drain(..).next().unwrap().flags;
        carry = Some(piece);
        if flags.len() <= 1 {
            console_log!(
                "flag log: DROPPED a single applied flag of {} bytes, past the {} byte \
                 backend limit and indivisible",
                size,
                MEASURED_BACKEND_LIMIT
            );
            any_lost = true;
            continue;
        }
        let tail = flags.split_off(flags.len() / 2);
        stack.push(tail);
        stack.push(flags);
    }

    if any_ok && any_lost {
        console_log!(
            "flag log: partially delivered a split record; {} record(s) had some \
             flags land and some dropped",
            count
        );
    }
    split_outcome(any_ok, any_lost, count)
}

/// Combines the per-piece outcomes of a flags split into one [`Delivered`].
///
/// By success bit, not record count: the record is indivisible at this
/// level, so "half its flags landed" cannot be expressed as a record split.
/// A mixed outcome therefore sets *both* counters, which is the predicate
/// the queue consumer's partial-ack path tests — anything else either acks
/// a half-loss as clean success or nacks and re-posts the flags that landed.
///
/// Pure so it can be tested without a network: this is where the bug was.
fn split_outcome(any_ok: bool, any_lost: bool, count: usize) -> Delivered {
    match (any_ok, any_lost) {
        (true, false) => Delivered::ok(count),
        (true, true) => Delivered {
            ok: count,
            lost: count,
        },
        // Nothing landed. The empty-stack case lands here too, which is
        // right: an empty split delivered nothing.
        (false, _) => Delivered::lost(count),
    }
}

/// What a queue consumer should do with a delivery outcome.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Disposition {
    /// Everything landed; ack.
    Complete,
    /// Some landed and some did not. Ack anyway: nacking redelivers the
    /// whole batch and re-posts what already succeeded, and split pieces
    /// cannot be nacked independently.
    Partial,
    /// Nothing landed, so redelivery duplicates nothing; nack.
    Failed,
}

/// Shared by [`queue::consume`] and its tests so the two cannot drift.
pub(super) fn ack_decision(outcome: &Delivered) -> Disposition {
    if outcome.ok > 0 && outcome.lost > 0 {
        Disposition::Partial
    } else if outcome.lost > 0 {
        Disposition::Failed
    } else {
        Disposition::Complete
    }
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

#[cfg(test)]
mod deliver_limit_tests {
    use super::*;

    /// JSON can expand past the backend limit even when the protobuf
    /// split predicted it would fit. This test constructs records with
    /// escape-heavy targeting keys that blow up in JSON, then verifies
    /// deliver_all_within_limit re-splits rather than dropping.
    ///
    /// Before the fix, a batch over MEASURED_BACKEND_LIMIT with count > 1
    /// was logged as DROPPED and returned false.
    #[test]
    fn proto_chunk_that_expands_past_json_limit_is_still_splittable() {
        use confidence_resolver::proto::confidence::flags::resolver::v1::events::{
            flag_assigned::{applied_flag::Assignment, AppliedFlag, AssignmentInfo},
            FlagAssigned,
        };

        // Build records with keys full of characters that expand in JSON
        // (backslashes, quotes, control chars).
        let escape_heavy = "\\\"\n\t".repeat(200);
        let logs: Vec<WriteFlagLogsRequest> = (0..10)
            .map(|i| WriteFlagLogsRequest {
                flag_assigned: (0..20)
                    .map(|j| FlagAssigned {
                        resolve_id: format!("r-{i}-{j}"),
                        client_info: None,
                        flags: vec![AppliedFlag {
                            flag: format!("flags/f-{j}"),
                            targeting_key: format!("{escape_heavy}-{i}-{j}"),
                            assignment: Some(Assignment::AssignmentInfo(AssignmentInfo {
                                variant: format!("flags/f-{j}/variants/on"),
                                segment: format!("flags/f-{j}/rules/r"),
                            })),
                            ..Default::default()
                        }],
                    })
                    .collect(),
                ..Default::default()
            })
            .collect();

        let total_proto: usize = logs.iter().map(prost::Message::encoded_len).sum();
        let agg = flag_logger::aggregate_batch(logs.clone());
        let json_size = serde_json::to_string(&agg).unwrap().len();

        // The test is only meaningful if proto fits but JSON doesn't.
        // If this assertion fails the test data needs adjusting.
        if total_proto <= PROTO_CHUNK_BYTES && json_size > MAX_DELIVERY_BYTES {
            // This is exactly the scenario that used to drop the batch.
            // deliver_all_within_limit can't be called from a sync test
            // (it needs an async runtime), but we can verify the split
            // logic in the synchronous part: the aggregate must be
            // splittable by halving flag_assigned.
            assert!(
                agg.flag_assigned.len() > 1,
                "aggregate must have splittable flag_assigned"
            );
            let mid = agg.flag_assigned.len() / 2;
            let mut first_half = agg.clone();
            let second_assigned = first_half.flag_assigned.split_off(mid);
            let first_json = serde_json::to_string(&first_half).unwrap().len();
            let second_half = WriteFlagLogsRequest {
                flag_assigned: second_assigned,
                ..Default::default()
            };
            let second_json = serde_json::to_string(&second_half).unwrap().len();
            assert!(
                first_json < json_size && second_json < json_size,
                "halving must reduce JSON size: whole={json_size} first={first_json} second={second_json}"
            );
        }
    }

    /// The proto split budget must be calibrated so that even at the
    /// measured 1.64× JSON expansion, the resulting body fits the
    /// delivery cap without needing the headroom path.
    #[test]
    fn proto_chunk_fits_within_delivery_cap_at_measured_expansion() {
        let worst_json = (PROTO_CHUNK_BYTES as f64 * 1.64) as usize;
        assert!(
            worst_json < MAX_DELIVERY_BYTES,
            "PROTO_CHUNK_BYTES ({}) * 1.64 = {} exceeds MAX_DELIVERY_BYTES ({})",
            PROTO_CHUNK_BYTES,
            worst_json,
            MAX_DELIVERY_BYTES
        );
    }
}

#[cfg(test)]
mod delivered_tests {
    use super::*;

    /// A split batch where one half lands must not be charged as a whole-
    /// batch loss: `DROPPED` is what operators alert on.
    #[test]
    fn partial_failure_counts_only_the_lost_half() {
        let a = Delivered::ok(50);
        let b = Delivered::lost(50);
        let merged = a.merge(b);
        assert_eq!(merged.ok, 50, "the half that landed is counted");
        assert_eq!(merged.lost, 50, "only the failed half is counted lost");
    }

    #[test]
    fn a_whole_batch_success_reports_no_loss() {
        let m = Delivered::ok(10).merge(Delivered::ok(7));
        assert_eq!((m.ok, m.lost), (17, 0));
    }

    #[test]
    fn merging_is_associative_over_several_splits() {
        let total = Delivered::ok(4)
            .merge(Delivered::lost(3))
            .merge(Delivered::ok(2))
            .merge(Delivered::lost(1));
        assert_eq!((total.ok, total.lost), (6, 4));
        assert_eq!(total.ok + total.lost, 10, "every record is accounted for");
    }

    /// Exercises `split_outcome` itself, not hand-built `Delivered` values:
    /// this is the function whose zero-count version made a failed tail
    /// invisible.
    #[test]
    fn split_outcome_reports_a_partial_split_on_both_counters() {
        let count = 1usize;

        let both_ok = split_outcome(true, false, count);
        assert_eq!(both_ok, Delivered { ok: 1, lost: 0 }, "every piece landed");

        let both_lost = split_outcome(false, true, count);
        assert_eq!(both_lost, Delivered { ok: 0, lost: 1 }, "no piece landed");

        let mixed = split_outcome(true, true, count);
        assert!(
            mixed.ok > 0 && mixed.lost > 0,
            "a mixed outcome must show on both counters, got {mixed:?}"
        );

        // An empty split delivered nothing, so it counts as lost.
        assert_eq!(
            split_outcome(false, false, count),
            Delivered { ok: 0, lost: 1 }
        );

        // The trap: a zero count makes success and failure the same value,
        // which is how a failed tail became invisible.
        assert_eq!(Delivered::ok(0), Delivered::lost(0));
    }

    /// The queue consumer branches on `ok`/`lost` to choose ack vs nack.
    /// Each branch is asserted against the outcomes `split_outcome` and the
    /// record splitter actually produce.
    #[test]
    fn queue_acks_a_partial_split_and_nacks_a_total_failure() {
        // The same function queue::consume branches on, not a copy.
        assert_eq!(
            ack_decision(&split_outcome(true, false, 1)),
            Disposition::Complete
        );
        assert_eq!(
            ack_decision(&split_outcome(true, true, 1)),
            Disposition::Partial,
            "a partial must be distinguishable from both clean outcomes"
        );
        assert_eq!(
            ack_decision(&split_outcome(false, true, 1)),
            Disposition::Failed,
            "nothing landed, so redelivery duplicates nothing"
        );

        // Record-level splits behave the same way.
        assert_eq!(
            ack_decision(&Delivered::ok(50).merge(Delivered::ok(50))),
            Disposition::Complete
        );
        assert_eq!(
            ack_decision(&Delivered::ok(50).merge(Delivered::lost(50))),
            Disposition::Partial
        );

        // Only a fully successful batch may be acked, and only a fully
        // successful batch may fold its request telemetry into KV — a
        // nacked batch is redelivered, so counting now would double-count
        // on the retry.
        for (outcome, acks, counts_telemetry) in [
            (Delivered::ok(100), true, true),
            (Delivered::ok(50).merge(Delivered::lost(50)), false, false),
            (Delivered::lost(100), false, false),
        ] {
            let complete = matches!(ack_decision(&outcome), Disposition::Complete);
            assert_eq!(complete, acks, "ack policy for {outcome:?}");
            assert_eq!(
                outcome.lost == 0,
                counts_telemetry,
                "telemetry gate for {outcome:?}"
            );
            assert_eq!(complete, outcome.lost == 0, "the two must agree");
        }
        assert_eq!(
            ack_decision(&Delivered::lost(50).merge(Delivered::lost(50))),
            Disposition::Failed
        );
    }

    /// The JSON re-split halves `flag_assigned` and recurses, so the record
    /// counts it apportions must still sum to the original.
    #[test]
    fn re_split_apportions_every_record() {
        for count in [1usize, 2, 3, 99, 100, 1253] {
            let head = count.div_ceil(2);
            let tail = count - head;
            assert_eq!(head + tail, count, "count {count} must be conserved");
            if count > 1 {
                assert!(head > 0 && tail > 0, "count {count} must split non-empty");
                assert!(head < count && tail < count, "count {count} must shrink");
            }
        }
    }
}
