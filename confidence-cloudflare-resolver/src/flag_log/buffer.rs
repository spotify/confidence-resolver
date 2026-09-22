//! In-isolate buffering, delivered straight to Confidence.
//!
//! Logs accumulate in isolate memory and are aggregated and POSTed to the
//! configured destinations. Nothing sits between the resolver and the
//! backend, so throughput scales with the number of isolates Cloudflare
//! runs, which scales with the traffic itself.
//!
//! That is what [`super::Sink::Queue`] cannot do: it bills per message and
//! caps around 5,000 messages a second per queue, so a high rate needs
//! shards and the cost tracks the record count.
//!
//! # Triggers
//!
//! A buffer is flushed on whichever comes first:
//!
//! * **size** — [`FLUSH_BYTES`] of encoded protobuf accumulated. This is the
//!   trigger that matters under load, and it is the one that bounds memory.
//! * **idle** — [`IDLE_MS`] with no new log, so a quiet isolate does not sit
//!   on records indefinitely.
//! * **age** — [`MAX_AGE_MS`] since the oldest log, so a slow trickle still
//!   drains on a predictable schedule.
//!
//! Sizing follows from the first trigger. At the ~9.7 KB per record measured
//! against a heavy flag set, [`FLUSH_BYTES`] is a few hundred records, so any
//! isolate above a modest request rate flushes on size and never reaches the
//! timers. The timers exist for the tail, where few records are at risk.
//!
//! # Swap, don't block
//!
//! Hitting the size trigger swaps in a fresh buffer and hands the old one to
//! the caller, which delivers it from `wait_until`. The request that happened
//! to fill the buffer does not pay the delivery round-trip, so buffering adds
//! no resolve latency.
//!
//! # Durability
//!
//! This is the least durable sink, deliberately. Cloudflare offers no
//! shutdown hook, so an isolate evicted while holding a buffer loses it
//! silently, and `wait_until` is not guaranteed to run. The exposure is
//! bounded by [`FLUSH_BYTES`] rather than by time.
//!
//! A delivery that fails transiently is retried by [`super::deliver`], but a
//! batch that exhausts its attempts is dropped rather than held: holding it
//! only enlarges the batch that fails next and the loss when the isolate
//! goes away. Riding out a backend outage is not something an in-memory sink
//! can do. Choose [`super::Sink::Queue`] where a durable hand-off matters
//! more than cost and throughput.
use confidence_resolver::proto::confidence::flags::resolver::v1::WriteFlagLogsRequest;
use prost::Message;
use std::cell::RefCell;
use worker::console_log;

/// Encoded protobuf to accumulate before flushing.
///
/// One flush should be one delivery. Anything larger is split again by
/// [`super::deliver_all_within_limit`], and every extra split is another
/// aggregate and another JSON body live at once — at 4 MB this trapped the
/// isolate with `memory access out of bounds` under load. Matching the
/// delivery budget keeps the common case to a single aggregate.
///
/// It also bounds what an eviction can take: this is the exposure, and it is
/// bounded by bytes rather than by the flush timers.
const FLUSH_BYTES: usize = super::PROTO_CHUNK_BYTES;

/// How much encoded protobuf the buffer may hold before records are shed.
///
/// Deliberately much larger than [`FLUSH_BYTES`]: a buffered record costs
/// roughly its own encoded size, while a delivery in flight costs about
/// 2.6x that (the records, plus an aggregate, plus a JSON body — measured,
/// JSON runs 1.64x the encoded size). Memory is therefore far better spent
/// here than on concurrency, and every byte of it is outage the sink rides
/// out without losing anything.
const MAX_BUFFER_BYTES: usize = 24 * 1024 * 1024;

const _: () = assert!(FLUSH_BYTES < MAX_BUFFER_BYTES);

/// Flush after this long with no new log.
const IDLE_MS: f64 = 1_000.0;

/// Flush this long after the oldest log, however slow the trickle.
const MAX_AGE_MS: f64 = 10_000.0;

/// Deliveries allowed in flight before the size trigger stops firing.
///
/// Each carries its own aggregate and JSON body, so without a cap a busy
/// isolate can have arbitrarily many live at once and exhaust memory however
/// small each one is. Past this the buffer keeps accumulating instead.
const MAX_IN_FLIGHT: usize = 4;

/// Deliveries in flight past which the isolate sheds load.
///
/// [`MAX_IN_FLIGHT`] alone is a soft cap: once the buffer reaches
/// [`MAX_BUFFER_BYTES`] it flushes regardless, because growing the buffer
/// without limit is worse. That escape hatch means a backend slow enough to
/// hold every delivery open lets the in-flight count climb without bound —
/// and retries make each one live longer. This is the actual ceiling: at it,
/// a flush is dropped rather than started, loudly, so memory stays bounded
/// by `MAX_BUFFER_BYTES + HARD_IN_FLIGHT * one delivery` instead of by how
/// badly the backend is behaving.
///
/// Small, for the reason above: slots are the expensive way to hold records.
/// Eight deliveries of [`FLUSH_BYTES`] plus a full buffer is roughly 45 MB,
/// which leaves the 128 MB isolate room for the resolver state and the
/// request being served.
const HARD_IN_FLIGHT: usize = 8;

const _: () = assert!(MAX_IN_FLIGHT < HARD_IN_FLIGHT);

/// How often the pending waiter re-checks the triggers.
const POLL_MS: u64 = 250;

/// Treat a waiter that has not checked in for this long as gone, and let
/// another request replace it. Several poll intervals, so a merely slow
/// isolate is not mistaken for a cancelled one.
const WAITER_STALE_MS: f64 = 5.0 * POLL_MS as f64;

thread_local! {
    static BUFFER: RefCell<Buffer> = const { RefCell::new(Buffer::new()) };
    /// Deliveries currently awaiting a response on this isolate.
    static IN_FLIGHT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

struct Buffer {
    /// Logs are held unaggregated and merged once at flush time, each
    /// paired with its encoded length.
    ///
    /// Aggregating on every offer would re-clone the whole accumulator per
    /// request, which is quadratic in the number of logs buffered. The
    /// length is cached because `encoded_len` walks the whole message, so
    /// recomputing it while chunking would re-walk the entire buffer on
    /// every flush.
    logs: Vec<(WriteFlagLogsRequest, usize)>,
    bytes: usize,
    /// When the oldest log landed, for the age trigger.
    first_ms: f64,
    /// When the newest log landed, for the idle trigger.
    last_ms: f64,
    /// When the current waiter last proved it was alive, or `None` if there
    /// is none.
    ///
    /// `wait_until` can be cancelled mid-sleep, which would otherwise strand
    /// the flag as set and leave the timers permanently dead on this isolate
    /// — records would then only ever leave on the size trigger, so a buffer
    /// that went quiet would never drain. Storing a heartbeat instead lets
    /// the next request take over a waiter that stopped running.
    waiting_since_ms: Option<f64>,
}

impl Buffer {
    const fn new() -> Self {
        Buffer {
            logs: Vec::new(),
            bytes: 0,
            first_ms: 0.0,
            last_ms: 0.0,
            waiting_since_ms: None,
        }
    }

    fn push(&mut self, log: WriteFlagLogsRequest, now_ms: f64) {
        if self.logs.is_empty() {
            self.first_ms = now_ms;
        }
        self.last_ms = now_ms;
        let len = log.encoded_len();
        self.bytes = self.bytes.saturating_add(len);
        self.logs.push((log, len));
    }

    /// Whether the accumulated size alone calls for a flush.
    fn size_due(&self) -> bool {
        self.bytes >= FLUSH_BYTES
    }

    /// Whether either timer has expired. Size is checked separately because
    /// it is the only trigger evaluated in the request path.
    fn time_due(&self, now_ms: f64) -> bool {
        if self.logs.is_empty() {
            return false;
        }
        now_ms - self.last_ms >= IDLE_MS || now_ms - self.first_ms >= MAX_AGE_MS
    }

    /// Whether a new waiter may start: either there is none, or the last one
    /// missed enough heartbeats that it is presumed cancelled.
    fn waiter_is_vacant(&self, now_ms: f64) -> bool {
        match self.waiting_since_ms {
            None => true,
            Some(since) => now_ms - since >= WAITER_STALE_MS,
        }
    }

    fn take(&mut self) -> Vec<WriteFlagLogsRequest> {
        self.bytes = 0;
        self.first_ms = 0.0;
        self.last_ms = 0.0;
        std::mem::take(&mut self.logs)
            .into_iter()
            .map(|(log, _)| log)
            .collect()
    }

    /// Splits off roughly [`FLUSH_BYTES`] worth, leaving the rest buffered.
    ///
    /// Used once the buffer has run past its flush size, so a backlog is
    /// drained in delivery-sized pieces instead of handing one delivery the
    /// entire backlog. Keeps every in-flight delivery the same size, which
    /// is what makes the memory bound predictable.
    fn take_chunk(&mut self) -> Vec<WriteFlagLogsRequest> {
        // Counted first, then drained in one move: popping from the front
        // one at a time is quadratic, and a full buffer is thousands of
        // records.
        let mut bytes = 0usize;
        let mut count = 0usize;
        for (_, len) in &self.logs {
            if count > 0 && bytes.saturating_add(*len) > FLUSH_BYTES {
                break;
            }
            bytes = bytes.saturating_add(*len);
            count += 1;
        }
        let taken: Vec<_> = self.logs.drain(..count).map(|(log, _)| log).collect();
        self.bytes = self.bytes.saturating_sub(bytes);
        if self.logs.is_empty() {
            self.first_ms = 0.0;
            self.last_ms = 0.0;
        }
        taken
    }
}

/// Holds one in-flight delivery slot for as long as it is alive.
struct InFlight;

impl InFlight {
    fn acquire() -> Self {
        IN_FLIGHT.with(|n| n.set(n.get() + 1));
        InFlight
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        IN_FLIGHT.with(|n| n.set(n.get().saturating_sub(1)));
    }
}

/// What to do with a buffer that has reached the size trigger.
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    /// Keep accumulating: the pipe is busy but the buffer is still small.
    Hold,
    /// Flush and deliver.
    Deliver,
    /// Flush and discard: too many deliveries are already stuck.
    Shed,
}

/// Pure so it can be tested off wasm32, where `console_log!` aborts.
fn decide(bytes: usize, in_flight: usize) -> Decision {
    if in_flight >= HARD_IN_FLIGHT && bytes >= MAX_BUFFER_BYTES {
        return Decision::Shed;
    }
    if in_flight >= MAX_IN_FLIGHT && bytes < MAX_BUFFER_BYTES {
        return Decision::Hold;
    }
    Decision::Deliver
}

/// Accumulates one log, returning the batch to deliver when a flush is due.
///
/// Runs in the request path, so it does no I/O: the returned batch is
/// delivered from `wait_until` by [`super::send`].
pub(super) fn offer(log: WriteFlagLogsRequest) -> Option<Vec<WriteFlagLogsRequest>> {
    let now_ms = js_sys::Date::now();
    BUFFER.with(|cell| {
        let mut buffer = cell.borrow_mut();
        buffer.push(log, now_ms);
        if !buffer.size_due() {
            return None;
        }
        let in_flight = IN_FLIGHT.with(|n| n.get());
        let decision = decide(buffer.bytes, in_flight);
        if decision == Decision::Hold {
            return None;
        }
        let batch = buffer.take_chunk();
        if decision == Decision::Shed {
            // Starting another delivery here is how the isolate runs out of
            // memory instead of just losing a batch.
            console_log!(
                "flag log buffer: SHED {} records, {} deliveries already in flight",
                batch.len(),
                in_flight
            );
            return None;
        }
        Some(batch)
    })
}

/// Delivers one flushed batch, splitting it to fit the backend.
pub(super) async fn deliver(logs: Vec<WriteFlagLogsRequest>) {
    if logs.is_empty() {
        return;
    }
    let records = logs.len();
    let started_ms = js_sys::Date::now();
    // Released on drop, not after the await: `wait_until` can be cancelled
    // mid-delivery, and a plain decrement after the await would be skipped,
    // leaking the slot. Enough leaks and the count sticks at the cap, the
    // size trigger stops firing, and the isolate only ever flushes on the
    // hard ceiling.
    let _slot = InFlight::acquire();
    let delivered = super::deliver_all_within_limit(logs).await;
    console_log!(
        "flag log buffer: flushed {} records in {}ms, delivered={}",
        records,
        (js_sys::Date::now() - started_ms) as u64,
        delivered
    );
    if !delivered {
        // Already retried by `deliver_all_within_limit`; this is the batch
        // having exhausted its attempts. Dropped rather than restored: a
        // failed batch put back would be re-sent alongside the next one,
        // enlarging the batch that fails and the loss when the isolate goes
        // away.
        console_log!("flag log buffer: DROPPED {} records", records);
    }
}

/// Drains the buffer on the idle and age triggers.
///
/// Called from `wait_until` after every request, and returns immediately
/// unless it becomes the isolate's single waiter. The size trigger is handled
/// in [`offer`]; this covers the tail, where traffic stops or trickles.
pub(super) async fn tick() {
    let claimed = BUFFER.with(|cell| {
        let mut buffer = cell.borrow_mut();
        let now_ms = js_sys::Date::now();
        if buffer.logs.is_empty() || !buffer.waiter_is_vacant(now_ms) {
            return false;
        }
        buffer.waiting_since_ms = Some(now_ms);
        true
    });
    if !claimed {
        return;
    }

    loop {
        worker::Delay::from(std::time::Duration::from_millis(POLL_MS)).await;
        let now_ms = js_sys::Date::now();
        let step = BUFFER.with(|cell| {
            let mut buffer = cell.borrow_mut();
            if buffer.logs.is_empty() {
                // Emptied by a size flush while we slept.
                buffer.waiting_since_ms = None;
                return Step::Done;
            }
            if !buffer.time_due(now_ms) {
                // Still waiting, and still alive.
                buffer.waiting_since_ms = Some(now_ms);
                return Step::Wait;
            }
            // Due. Respect the same in-flight backpressure the size trigger
            // does, or the timers become a way around it.
            let in_flight = IN_FLIGHT.with(|n| n.get());
            buffer.waiting_since_ms = Some(now_ms);
            match decide(buffer.bytes, in_flight) {
                Decision::Hold => Step::Wait,
                // Shedding is the size trigger's job; here the buffer is
                // draining rather than filling, so waiting is right.
                Decision::Shed => Step::Wait,
                // One chunk at a time, not the whole buffer: a backlog can
                // be MAX_BUFFER_BYTES and handing that to a single delivery
                // is the memory spike `take_chunk` exists to avoid.
                Decision::Deliver => Step::Deliver(buffer.take_chunk()),
            }
        });
        match step {
            Step::Done => return,
            Step::Wait => continue,
            // Keep the waiter for the next chunk: once traffic has stopped
            // there may be no further request to elect a replacement, so
            // returning here would strand the rest of the backlog.
            Step::Deliver(batch) => deliver(batch).await,
        }
    }
}

/// One iteration of the waiter loop.
enum Step {
    /// Nothing left; release the waiter.
    Done,
    /// Not due yet, or the pipe is busy.
    Wait,
    /// Deliver this chunk, then look again.
    Deliver(Vec<WriteFlagLogsRequest>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use confidence_resolver::flag_logger;

    use confidence_resolver::proto::confidence::flags::resolver::v1::events::{
        flag_assigned::{applied_flag::Assignment, AppliedFlag, AssignmentInfo},
        FlagAssigned,
    };

    /// One log carrying `n` assignments, shaped like a real exposure so
    /// `encoded_len` moves measurably per record and the size trigger is
    /// exercised against realistic bytes.
    fn log_with_assigns(n: usize) -> WriteFlagLogsRequest {
        WriteFlagLogsRequest {
            flag_assigned: (0..n)
                .map(|i| FlagAssigned {
                    resolve_id: format!("resolve-{i:08}"),
                    client_info: None,
                    flags: vec![AppliedFlag {
                        flag: format!("flags/flag-{i:04}"),
                        targeting_key: format!("targeting-key-{i:08}"),
                        assignment: Some(Assignment::AssignmentInfo(AssignmentInfo {
                            variant: format!("flags/flag-{i:04}/variants/treatment"),
                            segment: format!("flags/flag-{i:04}/rules/rule-0"),
                        })),
                        ..Default::default()
                    }],
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn holds_until_the_size_trigger() {
        let mut buffer = Buffer::new();
        buffer.push(log_with_assigns(1), 0.0);
        assert!(!buffer.size_due(), "one small log must not flush");
        assert_eq!(buffer.logs.len(), 1);
    }

    #[test]
    fn size_trigger_fires_at_the_budget() {
        let mut buffer = Buffer::new();
        let mut pushes = 0;
        while !buffer.size_due() {
            buffer.push(log_with_assigns(200), 0.0);
            pushes += 1;
            assert!(pushes < 100_000, "size trigger never fired");
        }
        assert!(buffer.bytes >= FLUSH_BYTES);
        let taken = buffer.take();
        assert_eq!(taken.len(), pushes);
        assert_eq!(buffer.bytes, 0, "take must reset the running size");
        assert!(buffer.logs.is_empty());
    }

    #[test]
    fn idle_trigger_fires_after_quiet() {
        let mut buffer = Buffer::new();
        buffer.push(log_with_assigns(1), 1_000.0);
        assert!(!buffer.time_due(1_000.0 + IDLE_MS - 1.0));
        assert!(buffer.time_due(1_000.0 + IDLE_MS));
    }

    /// A steady trickle keeps resetting the idle timer, so the age trigger is
    /// what guarantees those records still leave.
    #[test]
    fn age_trigger_fires_under_a_trickle() {
        let mut buffer = Buffer::new();
        buffer.push(log_with_assigns(1), 0.0);
        let mut now = 0.0;
        while now < MAX_AGE_MS {
            now += IDLE_MS / 2.0;
            buffer.push(log_with_assigns(1), now);
            if now < MAX_AGE_MS {
                assert!(
                    !buffer.time_due(now),
                    "idle timer should keep resetting at {now}"
                );
            }
        }
        assert!(buffer.time_due(MAX_AGE_MS));
    }

    #[test]
    fn empty_buffer_never_fires() {
        let buffer = Buffer::new();
        assert!(!buffer.time_due(1e9));
        assert!(!buffer.size_due());
    }

    #[test]
    fn take_resets_the_window() {
        let mut buffer = Buffer::new();
        buffer.push(log_with_assigns(1), 5_000.0);
        let _ = buffer.take();
        buffer.push(log_with_assigns(1), 6_000.0);
        assert_eq!(
            buffer.first_ms, 6_000.0,
            "age must be measured from the new oldest log"
        );
    }

    #[test]
    fn a_busy_pipe_holds_rather_than_piling_up() {
        assert_eq!(decide(FLUSH_BYTES, 0), Decision::Deliver);
        assert_eq!(decide(FLUSH_BYTES, MAX_IN_FLIGHT - 1), Decision::Deliver);
        assert_eq!(decide(FLUSH_BYTES, MAX_IN_FLIGHT), Decision::Hold);
    }

    /// Holding stops at the buffer ceiling: growing without bound is worse
    /// than one more concurrent delivery.
    #[test]
    fn the_buffer_ceiling_overrides_the_soft_cap() {
        assert_eq!(decide(MAX_BUFFER_BYTES, MAX_IN_FLIGHT), Decision::Deliver);
        assert_eq!(
            decide(MAX_BUFFER_BYTES, HARD_IN_FLIGHT - 1),
            Decision::Deliver
        );
    }

    /// ...but not past the hard ceiling, where memory is the bigger risk.
    #[test]
    fn the_hard_ceiling_sheds() {
        assert_eq!(decide(MAX_BUFFER_BYTES, HARD_IN_FLIGHT), Decision::Shed);
        // Still below the buffer ceiling, so there is no need to shed yet.
        assert_eq!(decide(FLUSH_BYTES, HARD_IN_FLIGHT), Decision::Hold);
    }

    /// A backlog must drain in delivery-sized pieces, so one delivery never
    /// gets handed the whole buffer.
    #[test]
    fn take_chunk_splits_a_backlog_and_leaves_the_rest() {
        let mut buffer = Buffer::new();
        while buffer.bytes < FLUSH_BYTES * 4 {
            buffer.push(log_with_assigns(20), 0.0);
        }
        let total_before = buffer.bytes;
        let total_records = buffer.logs.len();

        let chunk = buffer.take_chunk();
        assert!(!chunk.is_empty(), "must take something");
        let chunk_bytes: usize = chunk.iter().map(prost::Message::encoded_len).sum();
        assert!(
            chunk_bytes <= FLUSH_BYTES,
            "chunk {chunk_bytes} must fit the delivery budget {FLUSH_BYTES}"
        );
        assert!(!buffer.logs.is_empty(), "the rest stays buffered");
        assert_eq!(
            buffer.bytes,
            total_before - chunk_bytes,
            "running size must track what was removed"
        );

        // Draining repeatedly must conserve every record and empty the buffer.
        let mut drained = chunk.len();
        while !buffer.logs.is_empty() {
            drained += buffer.take_chunk().len();
        }
        assert_eq!(
            drained, total_records,
            "no record may be lost or duplicated"
        );
        assert_eq!(buffer.bytes, 0);
    }

    /// A single log bigger than the budget must still leave, not wedge.
    #[test]
    fn take_chunk_always_makes_progress() {
        let mut buffer = Buffer::new();
        buffer.push(log_with_assigns(20_000), 0.0);
        assert!(buffer.bytes > FLUSH_BYTES, "one oversized log");
        assert_eq!(buffer.take_chunk().len(), 1);
        assert!(buffer.logs.is_empty());
    }

    /// A cancelled delivery must not leak its in-flight slot, or the size
    /// trigger eventually stops firing on this isolate.
    #[test]
    fn an_abandoned_delivery_releases_its_slot() {
        IN_FLIGHT.with(|n| n.set(0));
        {
            let _a = InFlight::acquire();
            let _b = InFlight::acquire();
            assert_eq!(IN_FLIGHT.with(|n| n.get()), 2);
        }
        assert_eq!(
            IN_FLIGHT.with(|n| n.get()),
            0,
            "slots must be released even when the delivery never completes"
        );
    }

    /// A waiter whose `wait_until` was cancelled must not lock the timers
    /// out of this isolate for good.
    #[test]
    fn a_stale_waiter_can_be_replaced() {
        let mut buffer = Buffer::new();
        buffer.push(log_with_assigns(1), 0.0);
        assert!(buffer.waiter_is_vacant(0.0), "no waiter yet");
        buffer.waiting_since_ms = Some(0.0);
        assert!(
            !buffer.waiter_is_vacant(WAITER_STALE_MS - 1.0),
            "live waiter holds"
        );
        assert!(
            buffer.waiter_is_vacant(WAITER_STALE_MS),
            "cancelled waiter is replaceable"
        );
    }

    /// Aggregation is deferred to flush time; the buffer must hold the logs
    /// unmerged so that stays true.
    #[test]
    fn logs_are_held_unaggregated() {
        let mut buffer = Buffer::new();
        buffer.push(log_with_assigns(2), 0.0);
        buffer.push(log_with_assigns(3), 1.0);
        assert_eq!(buffer.logs.len(), 2);
        let merged = flag_logger::aggregate_batch(buffer.take());
        assert_eq!(
            merged.flag_assigned.len(),
            5,
            "aggregate_batch concatenates assignments"
        );
    }
}
