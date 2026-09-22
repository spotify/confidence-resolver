//! Best-effort statistics aggregation and bounded retries for failed exposures.
use confidence_resolver::{
    flag_logger, proto::confidence::flags::resolver::v1::WriteFlagLogsRequest,
};
use futures_util::future::{select, Either};
use std::{cell::RefCell, collections::VecDeque, future::Future, time::Duration};
use worker::console_log;

thread_local! {
    static BUFFER: RefCell<Buffer> = RefCell::new(Buffer::default());
}

const FLUSH_INTERVAL: Duration = Duration::from_millis(200);
const TARGET_MESSAGE_BYTES: usize = 60_000;
const MAX_MESSAGE_BYTES: usize = 120_000;
const MAX_BATCHES: usize = 4;

// Owns the shared flush/retry timer, never the first exposure publish.
struct Running;
impl Drop for Running {
    fn drop(&mut self) {
        BUFFER.with(|buffer| buffer.borrow_mut().running = false);
    }
}

/// Runs in this request's waitUntil. Exposure publication is independent of
/// buffer capacity, the statistics timer, and all other in-flight publishes.
pub(crate) async fn send(log: WriteFlagLogsRequest, queues: &[worker::Queue]) {
    send_with(log, worker::Delay::from, |json| async move {
        super::flag_log_queues::send_to_any(queues, &json, js_sys::Math::random()).await
    })
    .await;
}

async fn send_with<D, P>(
    mut log: WriteFlagLogsRequest,
    delay: impl Fn(Duration) -> D,
    publish: impl Fn(String) -> P,
) where
    D: Future<Output = ()>,
    P: Future<Output = bool>,
{
    if has_flag_assigns(&log) {
        BUFFER.with(|buffer| buffer.borrow_mut().attach_pending(&mut log));
        match serde_json::to_string(&log) {
            Ok(json) => {
                if publish(json).await {
                    return;
                }
                console_log!("exposure flag log publish failed; scheduling retry");
                BUFFER.with(|buffer| buffer.borrow_mut().push_exposure(log));
            }
            Err(e) => {
                console_log!("exposure flag log serialize failed: {:?}", e);
                return;
            }
        }
    } else {
        BUFFER.with(|buffer| {
            if !buffer.borrow_mut().push(log) {
                console_log!("statistics buffer full or message too large; statistics dropped");
            }
        });
    }

    let start = BUFFER.with(|buffer| {
        let mut buffer = buffer.borrow_mut();
        if buffer.running || buffer.is_empty() {
            return false;
        }
        buffer.running = true;
        true
    });
    if !start {
        return;
    }
    let _running = Running;
    let mut failures = 0;
    // Bound best-effort retry/flush work within this request's waitUntil.
    for _ in 0..16 {
        delay(FLUSH_INTERVAL * (1 << failures)).await;
        let json = BUFFER.with(|buffer| buffer.borrow_mut().start_send());
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
            BUFFER.with(|buffer| buffer.borrow_mut().in_flight = None);
            failures = 0;
        } else {
            BUFFER.with(|buffer| {
                let mut buffer = buffer.borrow_mut();
                if buffer.in_flight.as_ref().is_some_and(has_flag_assigns) {
                    buffer.exposure_attempts += 1;
                    if buffer.exposure_attempts == 3 {
                        buffer.in_flight = None;
                        console_log!("exposure retry limit reached; envelope dropped");
                    }
                }
            });
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

fn has_flag_assigns(log: &WriteFlagLogsRequest) -> bool {
    log.flag_assigned
        .iter()
        .any(|assignment| !assignment.flags.is_empty())
}

#[derive(Default)]
struct Buffer {
    pending: VecDeque<WriteFlagLogsRequest>,
    exposure_retries: VecDeque<WriteFlagLogsRequest>,
    in_flight: Option<WriteFlagLogsRequest>,
    exposure_attempts: usize,
    running: bool,
}

// Include the outer JSON-string encoding used by queue.send(json).
fn fits(log: &WriteFlagLogsRequest, limit: usize) -> bool {
    serde_json::to_string(log)
        .and_then(|json| serde_json::to_vec(&json))
        .is_ok_and(|bytes| bytes.len() <= limit)
}

impl Buffer {
    fn len(&self) -> usize {
        self.pending.len() + self.exposure_retries.len() + usize::from(self.in_flight.is_some())
    }

    fn push_exposure(&mut self, log: WriteFlagLogsRequest) {
        if !fits(&log, MAX_MESSAGE_BYTES) {
            console_log!("exposure too large for retry buffer; envelope dropped");
            return;
        }
        if self.len() >= MAX_BATCHES && self.pending.pop_back().is_some() {
            console_log!("statistics dropped to retain failed exposure");
        }
        if self.len() >= MAX_BATCHES {
            console_log!("exposure retry buffer full; envelope dropped");
            return;
        }
        self.exposure_retries.push_back(log);
    }

    fn push(&mut self, mut log: WriteFlagLogsRequest) -> bool {
        debug_assert!(!has_flag_assigns(&log), "exposures must bypass the buffer");
        log.flag_assigned.clear();
        if !fits(&log, MAX_MESSAGE_BYTES) {
            return false;
        }
        if let Some(last) = self.pending.back_mut() {
            let merged = flag_logger::aggregate_batch(vec![last.clone(), log.clone()]);
            if fits(&merged, TARGET_MESSAGE_BYTES) {
                *last = merged;
                return true;
            }
        }
        if self.len() >= MAX_BATCHES {
            return false;
        }
        self.pending.push_back(log);
        true
    }

    fn attach_pending(&mut self, exposure: &mut WriteFlagLogsRequest) {
        let mut index = 0;
        while index < self.pending.len() {
            let merged =
                flag_logger::aggregate_batch(vec![exposure.clone(), self.pending[index].clone()]);
            if fits(&merged, TARGET_MESSAGE_BYTES) {
                *exposure = merged;
                self.pending.remove(index);
            } else {
                index += 1;
            }
        }
        // Do not steal in-flight statistics: they may already have been delivered.
    }

    fn start_send(&mut self) -> Option<String> {
        // Called only between sends: an outstanding publish is never preempted.
        if !self.exposure_retries.is_empty()
            && self
                .in_flight
                .as_ref()
                .is_some_and(|log| !has_flag_assigns(log))
        {
            self.pending.push_front(self.in_flight.take().unwrap());
        }
        if self.in_flight.is_none() {
            self.in_flight = self
                .exposure_retries
                .pop_front()
                .or_else(|| self.pending.pop_front());
            self.exposure_attempts = 0;
        }
        self.in_flight.as_ref().map(|log| {
            serde_json::to_string(log).expect("buffer only accepts serializable envelopes")
        })
    }

    fn is_empty(&self) -> bool {
        self.in_flight.is_none() && self.pending.is_empty() && self.exposure_retries.is_empty()
    }
}
