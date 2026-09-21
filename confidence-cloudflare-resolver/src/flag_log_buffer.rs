//! Best-effort, isolate-local aggregation before queue publication.
use confidence_resolver::{
    flag_logger, proto::confidence::flags::resolver::v1::WriteFlagLogsRequest,
};
use futures_util::future::{select, Either};
use std::{
    cell::RefCell,
    collections::VecDeque,
    future::Future,
    task::{Poll, Waker},
    time::Duration,
};

thread_local! {
    static BUFFER: RefCell<Buffer> = RefCell::new(Buffer::default());
}

const FLUSH_INTERVAL: Duration = Duration::from_millis(200);

/// Reset scheduling on completion or cancellation; retained data can be retried
/// by the next request. No RefCell borrow is held across awaits.
struct Running;

impl Drop for Running {
    fn drop(&mut self) {
        BUFFER.with(|buffer| {
            let mut buffer = buffer.borrow_mut();
            buffer.running = false;
            buffer.wake = None;
        });
    }
}

/// Runs inside the caller's wait_until, not a detached interval. A quiet isolate
/// still gets its final flush, but prolonged failures do not keep it alive forever.
///
/// Envelopes with nonempty flag_assigned go straight to the queue, carrying
/// buffered information that fits. Only envelopes without exposures wait for
/// the periodic flush. All publishes share a sender to avoid concurrent retries.
pub(crate) async fn send(log: WriteFlagLogsRequest, queues: &[worker::Queue]) {
    let start = BUFFER.with(|buffer| {
        let mut buffer = buffer.borrow_mut();
        if !buffer.push(log) {
            worker::console_log!("flag log buffer full or message too large; incoming log dropped");
        }
        if buffer.running || buffer.is_empty() {
            return false;
        }
        buffer.running = true;
        true
    });
    if !start {
        return;
    }
    flush_with(worker::Delay::from, |json| async move {
        super::flag_log_queues::send_to_any(queues, &json, js_sys::Math::random()).await
    })
    .await;
}

async fn flush_with<D, P>(delay: impl Fn(Duration) -> D, publish: impl Fn(String) -> P)
where
    D: Future<Output = ()>,
    P: Future<Output = bool>,
{
    let _running = Running;
    let mut failures = 0;
    // Bound each request's background work below waitUntil's 30-second lifetime.
    // Under continuous load, a later request starts the next run.
    for _ in 0..16 {
        // Exposure envelopes bypass the batching delay. push() has already
        // attached buffered information where it fits, and start_send() selects
        // exposures ahead of statistics-only envelopes.
        let send_exposures_now = BUFFER.with(|buffer| buffer.borrow().exposure_flush_requested);
        if !send_exposures_now {
            let wake = futures_util::future::poll_fn(|cx| {
                BUFFER.with(|buffer| {
                    let mut buffer = buffer.borrow_mut();
                    if buffer.exposure_flush_requested {
                        Poll::Ready(())
                    } else {
                        buffer.wake = Some(cx.waker().clone());
                        Poll::Pending
                    }
                })
            });
            select(Box::pin(delay(FLUSH_INTERVAL)), Box::pin(wake)).await;
        }
        let json = BUFFER.with(|buffer| {
            let mut buffer = buffer.borrow_mut();
            buffer.exposure_flush_requested = false;
            buffer.wake = None;
            buffer.start_send()
        });
        let Some(json) = json else { break };
        let delivered = matches!(
            select(
                Box::pin(publish(json)),
                Box::pin(delay(Duration::from_secs(1)))
            )
            .await,
            Either::Left((true, _))
        );
        if delivered {
            BUFFER.with(|buffer| buffer.borrow_mut().acknowledge());
            failures = 0;
        } else {
            failures += 1;
            if failures == 3 {
                break;
            }
        }
        if BUFFER.with(|buffer| buffer.borrow().is_empty()) {
            break;
        }
    }
}

// Queue bodies are JSON strings, so account for the outer JSON string encoding too.
// Leave headroom below a 64 KB billing chunk, including queue metadata.
const TARGET_MESSAGE_BYTES: usize = 60_000;
// A single request may already be larger than the batching target. Retain it
// without merging rather than imposing the target as a new per-request limit.
const MAX_MESSAGE_BYTES: usize = 120_000;
const MAX_BATCHES: usize = 4;

/// Empty FlagAssigned envelopes are not exposures.
fn has_flag_assigns(log: &WriteFlagLogsRequest) -> bool {
    log.flag_assigned
        .iter()
        .any(|assignment| !assignment.flags.is_empty())
}

#[derive(Default)]
struct Buffer {
    pending: VecDeque<WriteFlagLogsRequest>,
    in_flight: Option<WriteFlagLogsRequest>,
    running: bool,
    wake: Option<Waker>,
    exposure_flush_requested: bool,
}

fn fits(log: &WriteFlagLogsRequest, limit: usize) -> bool {
    serde_json::to_string(log)
        .and_then(|json| serde_json::to_vec(&json))
        .is_ok_and(|bytes| bytes.len() <= limit)
}

impl Buffer {
    /// Returns false when this best-effort buffer cannot retain the incoming log.
    pub(crate) fn push(&mut self, mut log: WriteFlagLogsRequest) -> bool {
        log.flag_assigned
            .retain(|assignment| !assignment.flags.is_empty());
        let has_assignments = has_flag_assigns(&log);
        if !fits(&log, MAX_MESSAGE_BYTES) {
            return false;
        }
        if let Some(last) = self.pending.back_mut() {
            // Piggyback buffered information onto an incoming exposure envelope
            // before requesting its immediate publish below.
            let merged = flag_logger::aggregate_batch(vec![last.clone(), log.clone()]);
            if fits(&merged, TARGET_MESSAGE_BYTES) {
                *last = merged;
                self.notify(has_assignments);
                return true;
            }
        }
        if self.pending.len() + usize::from(self.in_flight.is_some()) >= MAX_BATCHES {
            // Exposure logs have priority over best-effort statistics. Never
            // evict a batch whose publish may currently be in progress.
            if !has_assignments {
                return false;
            }
            let Some(index) = self
                .pending
                .iter()
                .position(|batch| !has_flag_assigns(batch))
            else {
                return false;
            };
            self.pending.remove(index);
        }
        self.pending.push_back(log);
        // Only exposures trigger an immediate queue publish. Statistics-only
        // logs wait for the 200 ms periodic flush, even when split into batches.
        self.notify(has_assignments);
        true
    }

    fn notify(&mut self, has_assignments: bool) {
        self.exposure_flush_requested |= has_assignments;
        if self.exposure_flush_requested {
            if let Some(wake) = self.wake.take() {
                wake.wake();
            }
        }
    }

    /// Keep ownership until acknowledgement, including if the sending task is cancelled.
    pub(crate) fn start_send(&mut self) -> Option<String> {
        // Called only between publish attempts: an exposure can overtake a
        // failed statistics-only batch, but never interrupt an active send.
        if self
            .in_flight
            .as_ref()
            .is_none_or(|batch| !has_flag_assigns(batch))
        {
            if let Some(index) = self.pending.iter().position(has_flag_assigns) {
                let exposure = self.pending.remove(index).unwrap();
                if let Some(statistics) = self.in_flight.replace(exposure) {
                    self.pending.push_front(statistics);
                }
            }
        }
        if self.in_flight.is_none() {
            self.in_flight = self.pending.pop_front();
        }
        self.in_flight
            .as_ref()
            .map(|log| serde_json::to_string(log).expect("buffer only accepts serializable logs"))
    }

    pub(crate) fn acknowledge(&mut self) {
        self.in_flight = None;
        self.notify(self.pending.iter().any(has_flag_assigns));
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.in_flight.is_none() && self.pending.is_empty()
    }
}
