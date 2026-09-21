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

#[cfg(test)]
mod tests {
    use super::*;
    use confidence_resolver::proto::confidence::flags::resolver::v1::{
        events::{flag_assigned::AppliedFlag, FlagAssigned},
        telemetry_data::ResolveRate,
        TelemetryData,
    };
    use std::{cell::Cell, pin::Pin, rc::Rc, task::Context};

    fn log(count: u32, assignment: Option<&str>) -> WriteFlagLogsRequest {
        WriteFlagLogsRequest {
            telemetry_data: Some(TelemetryData {
                resolve_rate: vec![ResolveRate { count, reason: 1 }],
                ..Default::default()
            }),
            flag_assigned: assignment
                .map(|flag| FlagAssigned {
                    flags: vec![AppliedFlag {
                        flag: flag.into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                })
                .into_iter()
                .collect(),
            ..Default::default()
        }
    }

    fn decode(json: String) -> WriteFlagLogsRequest {
        serde_json::from_str(&json).unwrap()
    }

    fn large_statistics(id: u32) -> WriteFlagLogsRequest {
        use confidence_resolver::proto::confidence::flags::admin::v1::ClientResolveInfo;
        let mut request = log(id, None);
        request.client_resolve_info.push(ClientResolveInfo {
            client: "x".repeat(35_000),
            client_credential: format!("clients/{id}/credentials/test"),
            ..Default::default()
        });
        request
    }

    #[test]
    fn full_statistics_buffer_does_not_flush_early_and_yields_capacity_to_exposures() {
        let mut buffer = Buffer::default();
        for id in 1..=MAX_BATCHES as u32 {
            assert!(buffer.push(large_statistics(id)));
        }
        assert!(
            !buffer.exposure_flush_requested,
            "statistics must wait for the periodic flush"
        );
        assert!(buffer.push(log(10, Some(&"a".repeat(35_000)))));
        assert_eq!(buffer.pending.len(), MAX_BATCHES);
        assert!(has_flag_assigns(&decode(buffer.start_send().unwrap())));
        buffer.acknowledge();
        let remaining_counts: u32 = buffer
            .pending
            .iter()
            .map(|batch| batch.telemetry_data.as_ref().unwrap().resolve_rate[0].count)
            .sum();
        assert_eq!(
            remaining_counts,
            2 + 3 + 4,
            "only oldest statistics were evicted"
        );
    }

    #[test]
    fn statistics_cannot_displace_exposures() {
        let mut buffer = Buffer::default();
        for _ in 0..MAX_BATCHES {
            assert!(buffer.push(log(1, Some(&"a".repeat(35_000)))));
        }
        assert!(!buffer.push(large_statistics(1)));
        assert!(buffer.pending.iter().all(has_flag_assigns));
    }

    #[test]
    fn exposure_overtakes_failed_statistics_without_discarding_them() {
        let mut buffer = Buffer::default();
        buffer.push(log(1, None));
        let failed = buffer.start_send().unwrap();
        buffer.push(log(2, Some("flags/a")));
        assert!(has_flag_assigns(&decode(buffer.start_send().unwrap())));
        buffer.acknowledge();
        assert_eq!(buffer.start_send().unwrap(), failed);
    }

    #[derive(Default)]
    struct Clock(RefCell<Vec<(Duration, Rc<Cell<bool>>)>>);

    impl Clock {
        fn delay(&self, duration: Duration) -> impl Future<Output = ()> {
            let ready = Rc::new(Cell::new(false));
            self.0.borrow_mut().push((duration, ready.clone()));
            futures_util::future::poll_fn(move |_| {
                if ready.get() {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
        }

        fn tick(&self, duration: Duration) {
            for (d, ready) in self.0.borrow().iter() {
                if *d == duration {
                    ready.set(true);
                }
            }
        }
    }

    fn poll(future: Pin<&mut impl Future<Output = ()>>) -> Poll<()> {
        future.poll(&mut Context::from_waker(
            futures_util::task::noop_waker_ref(),
        ))
    }

    fn seed_buffer(assignment: Option<&str>) {
        BUFFER.with(|buffer| {
            let mut buffer = buffer.borrow_mut();
            *buffer = Buffer::default();
            buffer.push(log(1, assignment));
            buffer.running = true;
        });
    }

    #[test]
    fn statistics_burst_aggregates_into_one_publish() {
        seed_buffer(None);
        BUFFER.with(|buffer| {
            for _ in 1..100 {
                assert!(buffer.borrow_mut().push(log(1, None)));
            }
        });
        let clock = Clock::default();
        let sent = RefCell::new(Vec::new());
        let mut run = Box::pin(flush_with(
            |d| clock.delay(d),
            |json| {
                sent.borrow_mut().push(decode(json));
                std::future::ready(true)
            },
        ));
        assert!(poll(run.as_mut()).is_pending());
        assert!(sent.borrow().is_empty());
        clock.tick(FLUSH_INTERVAL);
        assert!(poll(run.as_mut()).is_ready());
        let sent = sent.borrow();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0].telemetry_data.as_ref().unwrap().resolve_rate[0].count,
            100
        );
    }

    #[test]
    fn single_statistics_request_flushes_after_silence() {
        seed_buffer(None);
        let clock = Clock::default();
        let sent = RefCell::new(Vec::new());
        let mut run = Box::pin(flush_with(
            |d| clock.delay(d),
            |json| {
                sent.borrow_mut().push(decode(json));
                std::future::ready(true)
            },
        ));
        assert!(poll(run.as_mut()).is_pending());
        assert!(sent.borrow().is_empty());
        // No new request is pushed or used to wake the sender.
        clock.tick(FLUSH_INTERVAL);
        assert!(poll(run.as_mut()).is_ready());
        let sent = sent.borrow();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0].telemetry_data.as_ref().unwrap().resolve_rate[0].count,
            1
        );
    }

    #[test]
    fn exposure_only_traffic_publishes_without_batching_delay() {
        seed_buffer(Some("flags/a"));
        let clock = Clock::default();
        let sent = RefCell::new(Vec::new());
        let mut run = Box::pin(flush_with(
            |d| clock.delay(d),
            |json| {
                sent.borrow_mut().push(decode(json));
                if sent.borrow().len() < 10 {
                    BUFFER.with(|buffer| {
                        assert!(buffer.borrow_mut().push(log(1, Some("flags/a"))));
                    });
                }
                std::future::ready(true)
            },
        ));
        // All ten publishes complete without advancing the clock.
        assert!(poll(run.as_mut()).is_ready());
        assert_eq!(sent.borrow().len(), 10);
        assert!(sent.borrow().iter().all(has_flag_assigns));
        BUFFER.with(|buffer| assert!(buffer.borrow().is_empty()));
    }

    #[test]
    fn failed_publish_recovers_without_losing_counts() {
        seed_buffer(Some("flags/a"));
        let clock = Clock::default();
        let attempts = RefCell::new(Vec::new());
        let mut run = Box::pin(flush_with(
            |d| clock.delay(d),
            |json| {
                attempts.borrow_mut().push(json);
                std::future::ready(attempts.borrow().len() > 1)
            },
        ));
        assert!(poll(run.as_mut()).is_pending());
        BUFFER.with(|buffer| assert!(buffer.borrow_mut().push(log(2, None))));
        clock.tick(FLUSH_INTERVAL);
        assert!(poll(run.as_mut()).is_pending());
        clock.tick(FLUSH_INTERVAL);
        assert!(poll(run.as_mut()).is_ready());
        let attempts = attempts.borrow();
        assert_eq!(attempts.len(), 3);
        assert_eq!(attempts[0], attempts[1]);
        let delivered: Vec<_> = attempts[1..].iter().cloned().map(decode).collect();
        assert_eq!(
            delivered
                .iter()
                .map(|log| log.telemetry_data.as_ref().unwrap().resolve_rate[0].count)
                .sum::<u32>(),
            3
        );
        assert_eq!(
            delivered.iter().filter(|log| has_flag_assigns(log)).count(),
            1
        );
        BUFFER.with(|buffer| assert!(buffer.borrow().is_empty()));
    }

    #[test]
    fn timer_flushes_without_another_request_and_merges_arrivals() {
        seed_buffer(None);
        let clock = Clock::default();
        let sent = RefCell::new(Vec::new());
        let mut run = Box::pin(flush_with(
            |d| clock.delay(d),
            |json| {
                sent.borrow_mut().push(decode(json));
                std::future::ready(true)
            },
        ));
        assert!(poll(run.as_mut()).is_pending());
        BUFFER.with(|buffer| {
            buffer.borrow_mut().push(log(2, None));
        });
        assert!(poll(run.as_mut()).is_pending());
        assert!(sent.borrow().is_empty());
        clock.tick(FLUSH_INTERVAL);
        assert!(poll(run.as_mut()).is_ready());
        assert_eq!(
            sent.borrow()[0]
                .telemetry_data
                .as_ref()
                .unwrap()
                .resolve_rate[0]
                .count,
            3
        );
        BUFFER.with(|buffer| {
            assert!(buffer.borrow().is_empty());
            assert!(!buffer.borrow().running);
        });
    }

    #[test]
    fn assignment_wakes_pending_timer() {
        seed_buffer(None);
        let clock = Clock::default();
        let sent = RefCell::new(Vec::new());
        let mut run = Box::pin(flush_with(
            |d| clock.delay(d),
            |json| {
                sent.borrow_mut().push(decode(json));
                std::future::ready(true)
            },
        ));
        assert!(poll(run.as_mut()).is_pending());
        BUFFER.with(|buffer| {
            buffer.borrow_mut().push(log(2, Some("flags/a")));
        });
        assert!(poll(run.as_mut()).is_ready());
        assert_eq!(sent.borrow().len(), 1);
        assert_eq!(sent.borrow()[0].flag_assigned.len(), 1);
    }

    #[test]
    fn failures_retry_on_timer_then_retain_and_release_scheduler() {
        seed_buffer(Some("flags/a"));
        let clock = Clock::default();
        let attempts = Cell::new(0);
        let mut run = Box::pin(flush_with(
            |d| clock.delay(d),
            |_| {
                attempts.set(attempts.get() + 1);
                std::future::ready(false)
            },
        ));
        assert!(poll(run.as_mut()).is_pending());
        assert_eq!(attempts.get(), 1);
        clock.tick(FLUSH_INTERVAL);
        assert!(poll(run.as_mut()).is_pending());
        assert_eq!(attempts.get(), 2);
        clock.tick(FLUSH_INTERVAL);
        assert!(poll(run.as_mut()).is_ready());
        assert_eq!(attempts.get(), 3);
        BUFFER.with(|buffer| {
            assert!(!buffer.borrow().is_empty());
            assert!(!buffer.borrow().running);
        });
    }

    #[test]
    fn exposure_interrupts_statistics_retry_backoff() {
        seed_buffer(None);
        let clock = Clock::default();
        let sent = RefCell::new(Vec::new());
        let mut run = Box::pin(flush_with(
            |d| clock.delay(d),
            |json| {
                sent.borrow_mut().push(decode(json));
                std::future::ready(sent.borrow().len() > 1)
            },
        ));
        assert!(poll(run.as_mut()).is_pending());
        clock.tick(FLUSH_INTERVAL);
        assert!(poll(run.as_mut()).is_pending());
        assert_eq!(sent.borrow().len(), 1);
        BUFFER.with(|buffer| {
            buffer.borrow_mut().push(log(2, Some("flags/a")));
        });
        assert!(poll(run.as_mut()).is_pending());
        assert_eq!(sent.borrow().len(), 2);
        assert!(has_flag_assigns(&sent.borrow()[1]));
        clock.tick(FLUSH_INTERVAL);
        assert!(poll(run.as_mut()).is_ready());
        assert_eq!(sent.borrow().len(), 3);
    }

    #[test]
    fn cancellation_during_publish_retains_batch_and_releases_scheduler() {
        seed_buffer(Some("flags/a"));
        let clock = Clock::default();
        let mut run = Box::pin(flush_with(|d| clock.delay(d), |_| std::future::pending()));
        assert!(poll(run.as_mut()).is_pending());
        drop(run);
        BUFFER.with(|buffer| {
            assert!(!buffer.borrow().running);
            assert!(buffer.borrow().in_flight.is_some());
        });
    }

    #[test]
    fn aggregates_telemetry_into_assignment_message() {
        let mut buffer = Buffer::default();
        assert!(buffer.push(log(2, None)));
        assert!(!buffer.exposure_flush_requested);
        assert!(buffer.push(log(3, Some("flags/a"))));
        assert!(buffer.exposure_flush_requested);
        let sent = decode(buffer.start_send().unwrap());
        assert_eq!(sent.telemetry_data.unwrap().resolve_rate[0].count, 5);
        assert_eq!(sent.flag_assigned[0].flags[0].flag, "flags/a");
        buffer.acknowledge();
        assert!(buffer.is_empty());
    }

    #[test]
    fn empty_assignment_does_not_trigger_early_flush() {
        let mut buffer = Buffer::default();
        let mut request = log(1, None);
        request.flag_assigned.push(FlagAssigned::default());
        buffer.push(request);
        assert!(!buffer.exposure_flush_requested);
        assert!(decode(buffer.start_send().unwrap())
            .flag_assigned
            .is_empty());
    }

    #[test]
    fn failed_or_cancelled_send_is_retained_without_losing_new_arrivals() {
        let mut buffer = Buffer::default();
        buffer.push(log(2, Some("flags/a")));
        let first = buffer.start_send().unwrap();
        buffer.push(log(3, None));
        assert_eq!(buffer.start_send().unwrap(), first);
        buffer.acknowledge();
        assert_eq!(
            decode(buffer.start_send().unwrap())
                .telemetry_data
                .unwrap()
                .resolve_rate[0]
                .count,
            3
        );
        buffer.acknowledge();
        assert!(buffer.is_empty());
    }

    #[test]
    fn bounds_memory_and_encoded_message_size_including_in_flight() {
        let mut buffer = Buffer::default();
        let large = "x".repeat(35_000);
        for _ in 0..MAX_BATCHES {
            assert!(buffer.push(log(1, Some(&large))));
        }
        let first = buffer.start_send().unwrap();
        assert!(serde_json::to_vec(&first).unwrap().len() <= MAX_MESSAGE_BYTES);
        assert!(!buffer.push(log(1, Some(&large))));
        buffer.acknowledge();
        assert!(buffer.push(log(1, Some(&large))));
        assert!(!buffer.push(log(1, Some(&"x".repeat(MAX_MESSAGE_BYTES)))));
    }
}
