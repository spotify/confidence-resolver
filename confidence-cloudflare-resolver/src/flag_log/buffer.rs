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
//! silently, and `wait_until` is not guaranteed to run.
//!
//! **How much an eviction can take.** In steady state the buffer flushes at
//! [`FLUSH_BYTES`] (768 KiB encoded), so that is the usual exposure. Under
//! backpressure it is far larger: while deliveries are stuck the buffer
//! keeps absorbing up to [`MAX_BUFFER_BYTES`] (12 MiB encoded, which
//! measured about 6x that in wasm heap), and every byte of it is lost if
//! the isolate goes away before the backend recovers. Size the risk against
//! `MAX_BUFFER_BYTES`, not `FLUSH_BYTES`.
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
/// In steady state this is also what an eviction can take. Under
/// backpressure the buffer absorbs up to [`MAX_BUFFER_BYTES`] instead, so
/// that is the worst-case exposure rather than this.
const FLUSH_BYTES: usize = super::PROTO_CHUNK_BYTES;

/// Records per flush, the count-based counterpart to [`FLUSH_BYTES`].
/// Keeps a flush of many tiny records bounded in memory, not just in bytes.
const FLUSH_RECORDS: usize = MAX_BUFFER_RECORDS / 16;

/// How much encoded protobuf the buffer may hold before records are shed.
///
/// Deliberately much larger than [`FLUSH_BYTES`]: a buffered record costs
/// roughly its own encoded size, while a delivery in flight costs about
/// 2.6x that (the records, plus an aggregate, plus a JSON body — measured,
/// JSON runs 1.64x the encoded size). Memory is therefore far better spent
/// here than on concurrency, and every byte of it is outage the sink rides
/// out without losing anything.
/// Real memory is roughly 2× `encoded_len` for string-heavy records
/// (struct padding + heap allocations for each `String`). At 12 MiB
/// encoded that is ~24 MiB of actual heap, plus up to 8 in-flight
/// deliveries at ~3 MiB each ≈ 48 MiB total — comfortable within
/// the 128 MiB isolate limit.
const MAX_BUFFER_BYTES: usize = 12 * 1024 * 1024;

/// Records the buffer may hold, whichever ceiling is hit first.
///
/// `encoded_len` is a wire measure and says nothing about what a record
/// costs in memory. Every entry is an inline `Entry` — measured at 336
/// bytes — regardless of how little it encodes to. A telemetry-only apply
/// log encodes to ~14 bytes, so the byte ceiling alone would admit ~900k of
/// them: **287 MiB of inline entries**, well past the 128 MiB isolate,
/// before any heap content, in-flight delivery or resolver state.
///
/// 20k entries is ~6.7 MiB of inline `Entry` plus their heap content. For
/// the string-heavy records the byte ceiling binds first (12 MiB at ~9.7 KB
/// each is ~1,300 records), so this only takes over for small ones — which
/// is exactly where the byte ceiling fails.
const MAX_BUFFER_RECORDS: usize = 20_000;

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

/// Below this much remaining budget, a standing-down waiter stops draining
/// rather than starting a delivery it cannot finish.
const MIN_DRAIN_BUDGET_MS: f64 = 1_000.0;

/// Remaining budget at which a waiter stands down and drains its tail.
///
/// Comfortably above [`MIN_DRAIN_BUDGET_MS`] so the drain has real time to
/// place records rather than discovering it is already too late. Measured
/// delivery is a few hundred ms, so this is several deliveries' worth.
const RETIRE_AT_REMAINING_MS: f64 = 6_000.0;

const _: () = assert!(RETIRE_AT_REMAINING_MS > MIN_DRAIN_BUDGET_MS);

/// How often the pending waiter re-checks the triggers.
const POLL_MS: u64 = 250;

thread_local! {
    static BUFFER: RefCell<Buffer> = const { RefCell::new(Buffer::new()) };
    /// Deliveries currently awaiting a response on this isolate.
    static IN_FLIGHT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

struct Buffer {
    /// Logs are held unaggregated and merged once at flush time.
    ///
    /// Aggregating on every offer would re-clone the whole accumulator per
    /// request, which is quadratic in the number of logs buffered.
    logs: Vec<Entry>,
    bytes: usize,
    /// Sequence to assign the next arrival.
    next_seq: u64,
    /// Whether a waiter is currently polling the timers on this isolate.
    ///
    /// Released by [`Waiter`]'s `Drop`, so a `wait_until` cancelled mid-poll
    /// frees it automatically. A heartbeat was the obvious alternative and
    /// is wrong: a delivery with retries runs for seconds, which is longer
    /// than any sane staleness window, so a second waiter would be elected
    /// while the first was simply busy.
    waiter_active: bool,
}

/// One buffered log, with everything the triggers need about it.
struct Entry {
    log: WriteFlagLogsRequest,
    /// Monotonic per-isolate arrival order.
    ///
    /// A retiring waiter must drain only what it already owned. A saved
    /// *count* cannot express that: if a size-triggered flush removes some
    /// of the original records first, the count still says N and the drain
    /// takes N newer ones instead. A boundary on this sequence identifies
    /// the same records no matter what else consumed from the buffer.
    seq: u64,
    /// Cached because `encoded_len` walks the whole message, so recomputing
    /// it while chunking would re-walk the buffer on every flush.
    len: usize,
    /// Cached so the age window survives a partial drain: `take_chunk`
    /// leaves the tail behind, and a window stored on the buffer would go on
    /// measuring from a record that has already left, firing the age trigger
    /// immediately on records that are in fact fresh.
    at_ms: f64,
}

impl Buffer {
    const fn new() -> Self {
        Buffer {
            logs: Vec::new(),
            bytes: 0,
            next_seq: 0,
            waiter_active: false,
        }
    }

    fn push(&mut self, log: WriteFlagLogsRequest, now_ms: f64) {
        let len = log.encoded_len();
        self.bytes = self.bytes.saturating_add(len);
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        self.logs.push(Entry {
            log,
            len,
            at_ms: now_ms,
            seq,
        });
    }

    /// When the oldest still-buffered log landed.
    fn first_ms(&self) -> Option<f64> {
        self.logs.first().map(|e| e.at_ms)
    }

    /// When the newest log landed.
    fn last_ms(&self) -> Option<f64> {
        self.logs.last().map(|e| e.at_ms)
    }

    /// Whether the accumulated size alone calls for a flush.
    fn size_due(&self) -> bool {
        self.bytes >= FLUSH_BYTES || self.logs.len() >= FLUSH_RECORDS
    }

    /// Whether the buffer has hit either absorb ceiling.
    fn at_ceiling(&self) -> bool {
        self.bytes >= MAX_BUFFER_BYTES || self.logs.len() >= MAX_BUFFER_RECORDS
    }

    /// Whether either timer has expired. Size is checked separately because
    /// it is the only trigger evaluated in the request path.
    fn time_due(&self, now_ms: f64) -> bool {
        let (Some(first), Some(last)) = (self.first_ms(), self.last_ms()) else {
            return false;
        };
        now_ms - last >= IDLE_MS || now_ms - first >= MAX_AGE_MS
    }

    fn waiter_is_vacant(&self) -> bool {
        !self.waiter_active
    }

    #[cfg(test)]
    fn take(&mut self) -> Vec<WriteFlagLogsRequest> {
        self.bytes = 0;
        std::mem::take(&mut self.logs)
            .into_iter()
            .map(|e| e.log)
            .collect()
    }

    /// Everything at once. Tests only: production drains in bounded chunks
    /// so the slot cap and chunk bound still apply.
    #[cfg(test)]
    fn take_all(&mut self) -> Vec<WriteFlagLogsRequest> {
        self.bytes = 0;
        std::mem::take(&mut self.logs)
            .into_iter()
            .map(|e| e.log)
            .collect()
    }

    /// A chunk drawn only from records that arrived before `boundary`.
    ///
    /// Identifies the same set however the buffer changes underneath: a
    /// size-triggered flush removing some of them shrinks what this
    /// returns, rather than letting it reach forward into newer arrivals
    /// the way a saved count would.
    fn take_chunk_before(&mut self, boundary: u64) -> Vec<WriteFlagLogsRequest> {
        let mut bytes = 0usize;
        let mut count = 0usize;
        for entry in &self.logs {
            if entry.seq >= boundary {
                break;
            }
            if count > 0
                && (bytes.saturating_add(entry.len) > FLUSH_BYTES || count >= FLUSH_RECORDS)
            {
                break;
            }
            bytes = bytes.saturating_add(entry.len);
            count += 1;
        }
        let taken: Vec<_> = self.logs.drain(..count).map(|e| e.log).collect();
        self.bytes = self.bytes.saturating_sub(bytes);
        taken
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
        for entry in &self.logs {
            if count > 0
                && (bytes.saturating_add(entry.len) > FLUSH_BYTES || count >= FLUSH_RECORDS)
            {
                break;
            }
            bytes = bytes.saturating_add(entry.len);
            count += 1;
        }
        let taken: Vec<_> = self.logs.drain(..count).map(|e| e.log).collect();
        self.bytes = self.bytes.saturating_sub(bytes);
        taken
    }
}

/// Holds the isolate's waiter role for as long as it is alive.
struct Waiter;

impl Waiter {
    /// `Some` if the role was free and is now claimed.
    fn claim() -> Option<Self> {
        BUFFER.with(|cell| {
            let mut buffer = cell.borrow_mut();
            if buffer.logs.is_empty() || !buffer.waiter_is_vacant() {
                return None;
            }
            buffer.waiter_active = true;
            Some(Waiter)
        })
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        BUFFER.with(|cell| cell.borrow_mut().waiter_active = false);
    }
}

/// Holds one in-flight delivery slot for as long as it is alive.
///
/// Acquired in the same synchronous block that decides to remove a chunk,
/// never later. A chunk that has left the buffer no longer counts against
/// `MAX_BUFFER_BYTES`, so if it does not immediately count against
/// `IN_FLIGHT` it is bounded by neither: `offer` hands its batch to
/// `wait_until`, and every request task already runnable in the same tick
/// would admit against the same stale count before any of their delivery
/// futures got polled. Measured: 13 concurrent deliveries against a
/// `HARD_IN_FLIGHT` of 8.
pub(crate) struct InFlight;

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
fn decide(at_ceiling: bool, in_flight: usize) -> Decision {
    if in_flight >= HARD_IN_FLIGHT && at_ceiling {
        return Decision::Shed;
    }
    if in_flight >= MAX_IN_FLIGHT && !at_ceiling {
        return Decision::Hold;
    }
    Decision::Deliver
}

/// Accumulates one log, returning the batch to deliver when a flush is due.
///
/// Runs in the request path, so it does no I/O: the returned batch is
/// delivered from `wait_until` by [`super::send`].
pub(super) fn offer(log: WriteFlagLogsRequest) -> Option<(Vec<WriteFlagLogsRequest>, InFlight)> {
    offer_at(log, js_sys::Date::now())
}

/// Takes `now_ms` so the admission path can be tested off wasm32, where
/// `js_sys::Date::now()` aborts.
fn offer_at(
    log: WriteFlagLogsRequest,
    now_ms: f64,
) -> Option<(Vec<WriteFlagLogsRequest>, InFlight)> {
    BUFFER.with(|cell| {
        let mut buffer = cell.borrow_mut();
        buffer.push(log, now_ms);
        if !buffer.size_due() {
            return None;
        }
        let in_flight = IN_FLIGHT.with(|n| n.get());
        let decision = decide(buffer.at_ceiling(), in_flight);
        if decision == Decision::Hold {
            return None;
        }
        if decision == Decision::Shed {
            // Starting another delivery here is how the isolate runs out of
            // memory instead of just losing a batch. No slot is reserved:
            // shedding discards, it does not deliver.
            let batch = buffer.take_chunk();
            console_log!(
                "flag log buffer: SHED {} records, {} deliveries already in flight",
                batch.len(),
                in_flight
            );
            return None;
        }
        // Reserved before the chunk leaves the buffer, and travelling with
        // it: the caller detaches this batch into `wait_until`, and until
        // the slot is held the batch counts against no bound at all.
        let slot = InFlight::acquire();
        Some((buffer.take_chunk(), slot))
    })
}

/// Delivers one flushed batch, splitting it to fit the backend.
///
/// Takes the [`InFlight`] slot its caller reserved when it decided to remove
/// the chunk, and holds it until the delivery finishes. Acquiring one here
/// instead would leave the batch uncounted for as long as this future sits
/// unpolled in `wait_until`.
pub(super) async fn deliver(
    logs: Vec<WriteFlagLogsRequest>,
    slot: InFlight,
    deadline: super::Deadline,
) {
    if logs.is_empty() {
        return;
    }
    let records = logs.len();
    // Merge telemetry from every record: each carries a per-request delta
    // (latency histogram, resolve-rate counters, dedup counters), not a
    // snapshot. Taking only the last one would undercount by a factor of
    // the batch size.
    let telemetry = {
        use confidence_resolver::telemetry::TelemetrySnapshot;
        let mut snap = TelemetrySnapshot::default();
        for log in &logs {
            if let Some(td) = &log.telemetry_data {
                snap.accumulate_delta(td);
            }
        }
        snap
    };
    let started_ms = js_sys::Date::now();
    // Released on drop, not after the await: `wait_until` can be cancelled
    // mid-delivery, and a plain decrement after the await would be skipped,
    // leaking the slot. Enough leaks and the count sticks at the cap, the
    // size trigger stops firing, and the isolate only ever flushes on the
    // hard ceiling. Dropping this future before it is ever polled releases
    // the slot the same way.
    let _slot = slot;
    let outcome = super::deliver_all_within_limit(logs, deadline).await;
    let elapsed = (js_sys::Date::now() - started_ms) as u64;
    console_log!(
        "flag log buffer: flushed {} records in {}ms, delivered={} lost={}",
        records,
        elapsed,
        outcome.ok,
        outcome.lost
    );
    super::update_metrics(&telemetry, outcome.lost == 0).await;
    if outcome.lost > 0 {
        // Counts only what was actually lost: a split batch where one half
        // landed must not be reported as a whole-batch loss.
        console_log!("flag log buffer: DROPPED {} records", outcome.lost);
    }
}

/// Drains what it can before a waiter stands down.
///
/// Respects the same admission rules as every other path: one bounded chunk
/// at a time, never past [`HARD_IN_FLIGHT`], and never past the
/// invocation's deadline. `take_all` is deliberately not used — handing a
/// whole 12 MiB backlog to one delivery bypasses both the slot cap and the
/// chunk bound that make the memory model hold.
///
/// The caller drops its [`Waiter`] *before* calling this, so a request
/// arriving during the drain can elect a successor for anything that lands
/// in the fresh buffer.
async fn drain_before_standing_down(reason: &str, deadline: super::Deadline) {
    // Only the records this waiter already owned. Anything arriving during
    // the drain belongs to a newer invocation with its own budget: pulling
    // those in would deliver them on this waiter's nearly-spent deadline,
    // and once it is too spent to fund an attempt they are taken from the
    // buffer and dropped without a fetch ever being made.
    // Everything already buffered, identified by arrival order rather than
    // by a count, so a concurrent size flush cannot shift the boundary onto
    // newer records.
    let boundary = BUFFER.with(|c| c.borrow().next_seq);
    let mut delivered = 0usize;

    loop {
        // Take nothing unless the remaining budget can fund a real attempt
        // *and* the delivery it starts. Leaving records buffered for a
        // fresh waiter is strictly better than removing them to drop them.
        if deadline.remaining_ms() < MIN_DRAIN_BUDGET_MS {
            break;
        }
        let batch = BUFFER.with(|cell| {
            let mut buffer = cell.borrow_mut();
            if buffer.logs.is_empty() {
                return None;
            }
            let in_flight = IN_FLIGHT.with(|n| n.get());
            // Same admission as the size trigger: do not start a delivery
            // the caps would have refused.
            if in_flight >= MAX_IN_FLIGHT && !buffer.at_ceiling() {
                return None;
            }
            if in_flight >= HARD_IN_FLIGHT {
                return None;
            }
            let slot = InFlight::acquire();
            Some((buffer.take_chunk_before(boundary), slot))
        });
        match batch {
            Some((chunk, slot)) if !chunk.is_empty() => {
                delivered += chunk.len();
                deliver(chunk, slot, deadline).await;
            }
            _ => break,
        }
    }

    let (left, bytes) = BUFFER.with(|c| {
        let b = c.borrow();
        (b.logs.len(), b.bytes)
    });
    if left > 0 {
        console_log!(
            "flag log buffer: waiter {} after draining {} records; {} records \
             ({} bytes) left buffered for the next waiter",
            reason,
            delivered,
            left,
            bytes
        );
    } else if delivered > 0 {
        console_log!(
            "flag log buffer: waiter {} after draining {} records, buffer empty",
            reason,
            delivered
        );
    }
}

/// Drains the buffer on the idle and age triggers.
///
/// Called from `wait_until` after every request, and returns immediately
/// unless it becomes the isolate's single waiter. The size trigger is handled
/// in [`offer`]; this covers the tail, where traffic stops or trickles.
pub(super) async fn tick(deadline: super::Deadline) {
    let Some(waiter) = Waiter::claim() else {
        return;
    };

    loop {
        worker::Delay::from(std::time::Duration::from_millis(POLL_MS)).await;
        let now_ms = js_sys::Date::now();

        // Budget first, before the idle/age check. Deferring it meant a
        // waiter with no budget kept polling while not-yet-due, then
        // discovered it was out of budget with under a second left — too
        // little for the drain to place anything, so the tail was stranded
        // with no owner. Standing down at RETIRE_AT_REMAINING_MS leaves
        // real time to deliver it.
        if deadline.remaining_ms() < RETIRE_AT_REMAINING_MS {
            let has_records = BUFFER.with(|c| !c.borrow().logs.is_empty());
            if !has_records {
                return;
            }
            // Release the role before draining, so a request arriving
            // during it can elect a successor for what it buffers.
            drop(waiter);
            drain_before_standing_down("standing down on budget", deadline).await;
            return;
        }

        let step = BUFFER.with(|cell| {
            let mut buffer = cell.borrow_mut();
            if buffer.logs.is_empty() {
                // Emptied by a size flush while we slept.
                return Step::Done;
            }
            if !buffer.time_due(now_ms) {
                return Step::Wait;
            }
            // Due. But taking a chunk this waiter cannot pay to deliver is
            // how records are removed from the buffer and dropped without a
            // fetch: `deliver` refuses an attempt below MIN_ATTEMPT_MS and
            // the chunk is already gone. The waiter's own deadline depletes
            // as it runs, so this is reached in ordinary operation, not only
            // at retirement. Leave the records for a waiter with budget.
            // Respect the same in-flight backpressure the size trigger does,
            // or the timers become a way around it.
            let in_flight = IN_FLIGHT.with(|n| n.get());
            match decide(buffer.at_ceiling(), in_flight) {
                Decision::Hold => Step::Wait,
                // Shed here too. Leaving it to the size trigger strands the
                // buffer when traffic stops: no further `offer` arrives, the
                // waiter gives up, and the whole MAX_BUFFER_BYTES is lost to
                // the next eviction with no marker in the logs.
                Decision::Shed => Step::Shed(buffer.take_chunk()),
                // One chunk at a time, not the whole buffer: a backlog can
                // be MAX_BUFFER_BYTES and handing that to a single delivery
                // is the memory spike `take_chunk` exists to avoid.
                Decision::Deliver => {
                    // Reserve before the chunk leaves the buffer, so a
                    // concurrent `offer` sees this delivery immediately.
                    let slot = InFlight::acquire();
                    Step::Deliver(buffer.take_chunk(), slot)
                }
            }
        });
        match step {
            Step::Done => return,
            Step::Shed(batch) => {
                // Keep shedding down to FLUSH_BYTES, not just under
                // MAX_BUFFER_BYTES. Discarding one chunk already takes
                // `bytes` below the ceiling, so `decide` would answer Hold
                // on the next pass and the remaining ~11 MiB would sit
                // there until the waiter gave up and an eviction took it.
                let mut shed_records = batch.len();
                let mut shed_chunks = 1usize;
                loop {
                    let more = BUFFER.with(|cell| {
                        let mut buffer = cell.borrow_mut();
                        if buffer.logs.is_empty() || !buffer.size_due() {
                            return None;
                        }
                        Some(buffer.take_chunk())
                    });
                    match more {
                        Some(chunk) => {
                            shed_records += chunk.len();
                            shed_chunks += 1;
                        }
                        None => break,
                    }
                }
                console_log!(
                    "flag log buffer: SHED {} records in {} chunks from the timer \
                     path, {} deliveries stuck",
                    shed_records,
                    shed_chunks,
                    IN_FLIGHT.with(|n| n.get())
                );
                continue;
            }
            Step::Wait => {
                // Deliberately no give-up here. Waiting is the correct
                // response to "not due yet" and to "pipe busy" alike, and
                // standing down early strands records that become
                // deliverable the moment the backend recovers — measured:
                // 30 records left with no waiter, which a single later
                // request then flushed successfully. The deadline is the
                // only thing that ends a waiter.
                continue;
            }
            // Keep the waiter for the next chunk: once traffic has stopped
            // there may be no further request to elect a replacement, so
            // returning here would strand the rest of the backlog.
            Step::Deliver(batch, slot) => deliver(batch, slot, deadline).await,
        }
    }
}

/// One iteration of the waiter loop.
enum Step {
    /// Nothing left; release the waiter.
    Done,
    /// Not due yet, or the pipe is busy.
    Wait,
    /// Deliver this chunk, then look again. Carries the slot reserved
    /// when the chunk was removed.
    Deliver(Vec<WriteFlagLogsRequest>, InFlight),
    /// Discard this chunk: too many deliveries are stuck to start another.
    Shed(Vec<WriteFlagLogsRequest>),
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
            buffer.first_ms(),
            Some(6_000.0),
            "age must be measured from the new oldest log"
        );
    }

    #[test]
    fn a_busy_pipe_holds_rather_than_piling_up() {
        // not at ceiling
        assert_eq!(decide(false, 0), Decision::Deliver);
        assert_eq!(decide(false, MAX_IN_FLIGHT - 1), Decision::Deliver);
        assert_eq!(decide(false, MAX_IN_FLIGHT), Decision::Hold);
    }

    /// Holding stops at the buffer ceiling: growing without bound is worse
    /// than one more concurrent delivery.
    #[test]
    fn the_buffer_ceiling_overrides_the_soft_cap() {
        assert_eq!(decide(true, MAX_IN_FLIGHT), Decision::Deliver);
        assert_eq!(decide(true, HARD_IN_FLIGHT - 1), Decision::Deliver);
    }

    /// ...but not past the hard ceiling, where memory is the bigger risk.
    #[test]
    fn the_hard_ceiling_sheds() {
        assert_eq!(decide(true, HARD_IN_FLIGHT), Decision::Shed);
        // Still below the buffer ceiling, so there is no need to shed yet.
        assert_eq!(decide(false, HARD_IN_FLIGHT), Decision::Hold);
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

    /// Admission must reserve the slot in the same synchronous step that
    /// removes the chunk.
    ///
    /// The bug: `offer` returned its chunk and left the increment to
    /// `deliver`, which only runs when its `wait_until` future is first
    /// polled. Every request task already runnable in the same executor
    /// tick therefore admitted against the same stale count — measured at
    /// 13 concurrent deliveries against a `HARD_IN_FLIGHT` of 8. A chunk
    /// that has left the buffer no longer counts against `MAX_BUFFER_BYTES`
    /// either, so in that window it was bounded by nothing at all.
    #[test]
    fn admission_reserves_before_the_chunk_leaves_the_buffer() {
        for (at_ceiling, cap) in [(true, HARD_IN_FLIGHT), (false, MAX_IN_FLIGHT)] {
            IN_FLIGHT.with(|n| n.set(0));
            // 96 request tasks decide and detach, and not one delivery
            // future is polled: nothing here is ever dropped.
            let mut detached = Vec::new();
            let mut refused = 0usize;
            for _ in 0..96 {
                let in_flight = IN_FLIGHT.with(|n| n.get());
                match decide(at_ceiling, in_flight) {
                    Decision::Deliver => detached.push(InFlight::acquire()),
                    Decision::Hold | Decision::Shed => refused += 1,
                }
            }
            assert_eq!(
                detached.len(),
                cap,
                "admissions must stop at the cap while every delivery is \
                 still unpolled (at_ceiling={at_ceiling})"
            );
            assert_eq!(refused, 96 - cap);
            assert_eq!(
                IN_FLIGHT.with(|n| n.get()),
                cap,
                "and be visible to the next offer"
            );

            // Dropping a batch that was never delivered — `wait_until`
            // cancelled before its first poll — must give the slots back.
            drop(detached);
            assert_eq!(
                IN_FLIGHT.with(|n| n.get()),
                0,
                "an unpolled admitted batch releases its slot on drop"
            );
        }
    }

    /// The same invariant driven through the real `offer` path, with the
    /// batches held undelivered exactly as `wait_until` holds them.
    ///
    /// Sized to stay below `MAX_BUFFER_BYTES` so admission is refused by
    /// `Hold` rather than `Shed`: the shed branch logs, and `console_log!`
    /// aborts off wasm32. At ~52 KB a record that is ~15 records per flush
    /// and 238 records before the ceiling.
    #[test]
    fn offer_stops_admitting_while_every_detached_batch_is_still_unpolled() {
        const OFFERS: usize = 200;
        IN_FLIGHT.with(|n| n.set(0));
        BUFFER.with(|c| *c.borrow_mut() = Buffer::new());

        // Nothing is awaited: each detached batch stays parked, standing in
        // for a `wait_until` future that has not been polled yet. A fixed
        // count rather than a "until the buffer fills" loop, so that late
        // reservation fails this test instead of spinning in it — with the
        // slots uncounted the buffer never accumulates.
        let mut parked = Vec::new();
        for i in 0..OFFERS {
            if let Some(batch) = offer_at(log_with_assigns(400), i as f64) {
                parked.push(batch);
            }
        }

        assert_eq!(
            parked.len(),
            MAX_IN_FLIGHT,
            "offer admitted {} concurrent deliveries against a soft cap of {} \
             while not one of them had been polled",
            parked.len(),
            MAX_IN_FLIGHT
        );
        assert_eq!(
            IN_FLIGHT.with(|n| n.get()),
            parked.len(),
            "every detached batch must already be counted"
        );
        assert!(
            BUFFER.with(|c| c.borrow().bytes) < MAX_BUFFER_BYTES,
            "sized to refuse by Hold, not Shed"
        );

        // Records refused admission stay buffered rather than being removed
        // and dropped on the floor.
        let buffered = BUFFER.with(|c| c.borrow().logs.len());
        let detached: usize = parked.iter().map(|(logs, _)| logs.len()).sum();
        assert_eq!(buffered + detached, OFFERS, "no record may be lost");

        // Freeing one slot lets exactly one more flush in, no more.
        parked.pop();
        for i in 0..20 {
            if let Some(batch) = offer_at(log_with_assigns(400), (OFFERS + i) as f64) {
                parked.push(batch);
            }
        }
        assert_eq!(parked.len(), MAX_IN_FLIGHT, "back at the cap, not past it");

        drop(parked);
        assert_eq!(IN_FLIGHT.with(|n| n.get()), 0);
        BUFFER.with(|c| *c.borrow_mut() = Buffer::new());
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

    /// The waiter role must be released even when the poll loop is dropped
    /// mid-flight, or the timers go dead on this isolate for good.
    #[test]
    fn the_waiter_role_is_released_on_drop() {
        BUFFER.with(|cell| {
            let mut b = cell.borrow_mut();
            *b = Buffer::new();
            b.push(log_with_assigns(1), 0.0);
        });
        {
            let first = Waiter::claim();
            assert!(first.is_some(), "role starts free");
            assert!(Waiter::claim().is_none(), "only one waiter at a time");
        }
        assert!(
            Waiter::claim().is_some(),
            "role must be free again once the waiter is dropped"
        );
        BUFFER.with(|cell| *cell.borrow_mut() = Buffer::new());
    }

    /// The age window must follow the records, not the buffer: after a
    /// partial drain it measures from the oldest log still held.
    #[test]
    fn a_partial_drain_moves_the_age_window() {
        let mut buffer = Buffer::new();
        // One record over the budget, so the chunk takes exactly it...
        buffer.push(log_with_assigns(20_000), 0.0);
        // ...leaving this much later one behind.
        buffer.push(log_with_assigns(1), 50_000.0);

        assert_eq!(buffer.take_chunk().len(), 1, "chunk takes only the first");
        assert_eq!(buffer.logs.len(), 1, "the fresh record stays");
        assert_eq!(
            buffer.first_ms(),
            Some(50_000.0),
            "age must measure from the surviving record, not a departed one"
        );
        // Just before the idle timer, so only the age trigger is in play.
        // With the window stored on the buffer this read as 50s old and
        // fired immediately; measured from the surviving record it is 999ms.
        assert!(
            !buffer.time_due(50_000.0 + IDLE_MS - 1.0),
            "a fresh record must not inherit a departed record's age"
        );
        assert!(
            buffer.time_due(50_000.0 + MAX_AGE_MS),
            "and must still age out on its own schedule"
        );
    }

    /// The timer path must drain a full buffer all the way down to the
    /// flush size. Shedding a single chunk leaves `bytes` under
    /// MAX_BUFFER_BYTES, so `decide` answers Hold and the rest is stranded.
    #[test]
    fn shedding_drains_down_to_the_flush_size() {
        let mut buffer = Buffer::new();
        while buffer.bytes < MAX_BUFFER_BYTES {
            buffer.push(log_with_assigns(200), 0.0);
        }
        let started = buffer.bytes;
        assert!(started >= MAX_BUFFER_BYTES);

        // One chunk is not enough: still far above the flush size.
        let _ = buffer.take_chunk();
        assert!(
            buffer.bytes > FLUSH_BYTES,
            "one chunk leaves {} bytes, still a large stranded buffer",
            buffer.bytes
        );

        // The loop in tick() keeps going until under the flush size.
        let mut chunks = 1;
        while !buffer.logs.is_empty() && buffer.bytes >= FLUSH_BYTES {
            let c = buffer.take_chunk();
            assert!(!c.is_empty(), "take_chunk must make progress");
            chunks += 1;
            assert!(chunks < 10_000, "shed loop must terminate");
        }
        assert!(
            buffer.bytes < FLUSH_BYTES,
            "drained to under the flush size"
        );
        assert!(
            chunks > 2,
            "a full buffer needs several chunks, got {chunks}"
        );
    }

    /// `encoded_len` is a wire measure, not a memory one. A buffer of tiny
    /// records must hit the count ceiling long before the byte ceiling
    /// would admit enough of them to exhaust the isolate.
    #[test]
    fn the_record_ceiling_bounds_memory_for_tiny_records() {
        let entry = std::mem::size_of::<Entry>();
        // What the byte ceiling alone would allow for a ~14 byte record.
        let by_bytes = MAX_BUFFER_BYTES / 14;
        let unbounded = by_bytes * entry;
        assert!(
            unbounded > 128 * 1024 * 1024,
            "the premise: {} tiny records would be {} MiB of inline entries",
            by_bytes,
            unbounded / (1024 * 1024)
        );
        // With the count ceiling the inline cost is bounded regardless.
        let bounded = MAX_BUFFER_RECORDS * entry;
        assert!(
            bounded < 32 * 1024 * 1024,
            "MAX_BUFFER_RECORDS={} at {}B each is {} MiB of inline entries",
            MAX_BUFFER_RECORDS,
            entry,
            bounded / (1024 * 1024)
        );
        // And the count ceiling actually engages: a buffer of empty records
        // reaches it while the byte total is still negligible.
        let mut buffer = Buffer::new();
        for _ in 0..MAX_BUFFER_RECORDS {
            buffer.push(WriteFlagLogsRequest::default(), 0.0);
        }
        assert!(buffer.at_ceiling(), "count ceiling must fire");
        assert!(
            buffer.bytes < MAX_BUFFER_BYTES,
            "byte ceiling would not have"
        );
    }

    /// A standing-down waiter must not bypass the slot cap. The drain
    /// loop's admission check is the same one `decide` applies, so at
    /// HARD_IN_FLIGHT it must refuse to start another delivery rather than
    /// creating a ninth.
    #[test]
    fn retirement_respects_the_hard_slot_cap() {
        // The predicate the drain loop uses, stated once here so a change
        // to it breaks this test.
        fn may_start(in_flight: usize, at_ceiling: bool) -> bool {
            if in_flight >= MAX_IN_FLIGHT && !at_ceiling {
                return false;
            }
            in_flight < HARD_IN_FLIGHT
        }
        assert!(may_start(0, false), "idle pipe delivers");
        assert!(
            may_start(MAX_IN_FLIGHT, true),
            "at ceiling, still under hard cap"
        );
        assert!(
            !may_start(MAX_IN_FLIGHT, false),
            "busy pipe below the ceiling must hold, not drain"
        );
        assert!(
            !may_start(HARD_IN_FLIGHT, true),
            "at the hard cap retirement must not create slot {}",
            HARD_IN_FLIGHT + 1
        );
        assert!(!may_start(HARD_IN_FLIGHT + 1, true), "nor beyond it");
    }

    /// The drain takes bounded chunks, so even a full buffer of tiny
    /// records cannot hand one delivery the whole backlog.
    #[test]
    fn retirement_drains_in_bounded_chunks() {
        let mut buffer = Buffer::new();
        for _ in 0..(FLUSH_RECORDS * 3) {
            buffer.push(log_with_assigns(1), 0.0);
        }
        let first = buffer.take_chunk();
        assert!(
            first.len() <= FLUSH_RECORDS,
            "a chunk took {} records, past the {} record bound",
            first.len(),
            FLUSH_RECORDS
        );
        assert!(!buffer.logs.is_empty(), "the rest stays for the next chunk");
    }

    /// A waiter must stand down while it can still deliver, not once it is
    /// too late. Checking the budget only after the idle/age check meant it
    /// discovered the shortfall with under a second left, so the drain
    /// placed nothing and the tail was stranded with no owner: 30 records
    /// that a single later request then flushed successfully.
    #[test]
    fn a_waiter_retires_with_time_left_to_drain() {
        assert!(
            RETIRE_AT_REMAINING_MS > MIN_DRAIN_BUDGET_MS,
            "retiring at {RETIRE_AT_REMAINING_MS}ms must leave more than the \
             {MIN_DRAIN_BUDGET_MS}ms the drain needs"
        );
        // The margin has to be worth something: several deliveries, not one
        // that barely starts.
        let margin = RETIRE_AT_REMAINING_MS - MIN_DRAIN_BUDGET_MS;
        assert!(
            margin >= 4_000.0,
            "only {margin}ms of usable drain time after standing down"
        );

        // Standing down happens strictly before the drain refuses to start.
        let at_retirement = super::super::Deadline::for_test(RETIRE_AT_REMAINING_MS);
        assert!(
            at_retirement.remaining_from(0.0) >= MIN_DRAIN_BUDGET_MS,
            "the drain must be able to run at the moment we stand down"
        );
    }

    /// A waiter must not stand down while it still has budget and the
    /// records are deliverable. Two clocks meant the shorter one retired it
    /// early: 30 records were left with no waiter after a backend recovered,
    /// and a single later request then flushed all of them successfully.
    /// The deadline is now the only thing that ends a waiter.
    #[test]
    fn only_the_deadline_ends_a_waiter() {
        // With budget remaining, a busy pipe is a reason to wait, never a
        // reason to stand down.
        assert_eq!(decide(false, MAX_IN_FLIGHT), Decision::Hold);
        assert_eq!(decide(false, HARD_IN_FLIGHT), Decision::Hold);

        // The budget check is what ends it, and it is expressed purely in
        // terms of the deadline — there is no second clock to disagree.
        let ample = super::super::Deadline::for_test(MIN_DRAIN_BUDGET_MS * 10.0);
        let spent = super::super::Deadline::for_test(MIN_DRAIN_BUDGET_MS / 2.0);
        assert!(
            ample.remaining_from(0.0) >= MIN_DRAIN_BUDGET_MS,
            "keeps going"
        );
        assert!(
            spent.remaining_from(0.0) < MIN_DRAIN_BUDGET_MS,
            "stands down"
        );
    }

    /// A retiring waiter must drain only the records it already owned.
    /// Records arriving mid-drain belong to a newer invocation with its own
    /// budget; consuming them on a nearly-spent deadline means they are
    /// removed from the buffer and dropped without a fetch.
    #[test]
    fn a_retiring_drain_does_not_consume_later_arrivals() {
        let mut buffer = Buffer::new();
        for _ in 0..5 {
            buffer.push(log_with_assigns(1), 0.0);
        }
        let boundary = buffer.next_seq;

        // A newer request lands during the drain.
        for _ in 0..3 {
            buffer.push(log_with_assigns(1), 100.0);
        }
        assert_eq!(buffer.logs.len(), 8);

        let mut drained = 0;
        loop {
            let chunk = buffer.take_chunk_before(boundary);
            if chunk.is_empty() {
                break;
            }
            drained += chunk.len();
        }
        assert_eq!(drained, 5, "drained exactly the owned records");
        assert_eq!(
            buffer.logs.len(),
            3,
            "later arrivals stay for a waiter with its own budget"
        );
    }

    /// The boundary must survive another task consuming the owned records
    /// first. A saved *count* could not: with the originals already gone it
    /// would still say five and take five newer records instead.
    #[test]
    fn the_boundary_survives_a_concurrent_flush() {
        let mut buffer = Buffer::new();
        for _ in 0..5 {
            buffer.push(log_with_assigns(1), 0.0);
        }
        let boundary = buffer.next_seq;

        // A size-triggered flush takes all five owned records...
        let flushed = buffer.take_chunk_before(boundary);
        assert_eq!(flushed.len(), 5);

        // ...and five newer ones arrive.
        for _ in 0..5 {
            buffer.push(log_with_assigns(1), 100.0);
        }
        assert_eq!(buffer.logs.len(), 5);

        // The retiring drain must now find nothing of its own.
        assert!(
            buffer.take_chunk_before(boundary).is_empty(),
            "the boundary must not reach forward into newer arrivals"
        );
        assert_eq!(buffer.logs.len(), 5, "newer records untouched");
    }

    /// Sequence numbers are assigned in arrival order and never reused, so
    /// a boundary always names the same set.
    #[test]
    fn sequence_numbers_are_monotonic() {
        let mut buffer = Buffer::new();
        for _ in 0..4 {
            buffer.push(log_with_assigns(1), 0.0);
        }
        let seqs: Vec<u64> = buffer.logs.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![0, 1, 2, 3]);
        let _ = buffer.take_chunk_before(2);
        buffer.push(log_with_assigns(1), 1.0);
        let after: Vec<u64> = buffer.logs.iter().map(|e| e.seq).collect();
        assert_eq!(after, vec![2, 3, 4], "draining does not reuse sequences");
    }

    /// A limited chunk still respects the ordinary flush bound.
    #[test]
    fn a_bounded_chunk_still_respects_the_flush_bound() {
        let mut buffer = Buffer::new();
        for _ in 0..(FLUSH_RECORDS * 2) {
            buffer.push(log_with_assigns(1), 0.0);
        }
        let chunk = buffer.take_chunk_before(u64::MAX);
        assert!(
            chunk.len() <= FLUSH_RECORDS,
            "took {} records, past the {} bound",
            chunk.len(),
            FLUSH_RECORDS
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

    /// The buffer ceiling must keep peak wasm heap well under the 128 MiB
    /// isolate limit.
    ///
    /// `encoded_len` measures protobuf wire bytes, not memory. Measured in
    /// the Workers runtime with `memory_size()` during a load test: a
    /// 12.59 MB encoded buffer (1,253 records, 58 flags each) drove peak
    /// wasm heap to 75.3 MB — a 6.0x multiplier that already includes the
    /// in-flight deliveries, since they allocate from the same heap.
    ///
    /// This would have failed at the original 24 MiB cap: 24 * 6.0 = 144 MB,
    /// past the 128 MB isolate limit outright.
    #[test]
    fn buffer_ceiling_fits_in_isolate_memory() {
        // Peak heap / peak encoded, measured end to end in the runtime.
        // Covers the buffer, every in-flight aggregate, and its JSON body.
        const MEASURED_PEAK_HEAP_RATIO: f64 = 6.0;
        let peak_heap = MAX_BUFFER_BYTES as f64 * MEASURED_PEAK_HEAP_RATIO;
        let isolate_limit = 128.0 * 1024.0 * 1024.0;
        // Leave room for the resolver state, the WASM module itself, and the
        // request being served.
        let headroom = 40.0 * 1024.0 * 1024.0;
        assert!(
            peak_heap < isolate_limit - headroom,
            "MAX_BUFFER_BYTES={} MiB implies {:.1} MB peak heap at the measured \
             {}x ratio, over the {:.0} MB safe limit",
            MAX_BUFFER_BYTES / (1024 * 1024),
            peak_heap / 1e6,
            MEASURED_PEAK_HEAP_RATIO,
            (isolate_limit - headroom) / 1e6
        );
    }
}
