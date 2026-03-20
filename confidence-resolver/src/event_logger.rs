use crossbeam_queue::SegQueue;

use crate::proto::google::{Struct, Timestamp};

pub struct TrackedEvent {
    pub event_definition: String,
    pub payload: Struct,
    pub event_time: Timestamp,
}

#[derive(Default)]
pub struct EventLogger {
    events: SegQueue<TrackedEvent>,
}

impl EventLogger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn track(&self, event: TrackedEvent) {
        self.events.push(event);
    }

    // TODO: Only drop events from memory after the provider confirms successful delivery.
    // Currently events are drained unconditionally — if the HTTP/gRPC send fails, they are lost.
    pub fn flush(&self) -> Vec<TrackedEvent> {
        let mut result = Vec::new();
        while let Some(event) = self.events.pop() {
            result.push(event);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(name: &str) -> TrackedEvent {
        TrackedEvent {
            event_definition: format!("eventDefinitions/{}", name),
            payload: Struct::default(),
            event_time: Timestamp {
                seconds: 1234,
                nanos: 0,
            },
        }
    }

    #[test]
    fn track_and_flush_single_event() {
        let logger = EventLogger::new();
        logger.track(make_event("purchase"));
        let events = logger.flush();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_definition, "eventDefinitions/purchase");
    }

    #[test]
    fn flush_drains_all_events() {
        let logger = EventLogger::new();
        logger.track(make_event("a"));
        logger.track(make_event("b"));
        logger.track(make_event("c"));
        let events = logger.flush();
        assert_eq!(events.len(), 3);
        // second flush returns empty
        assert!(logger.flush().is_empty());
    }

    #[test]
    fn empty_flush_returns_empty_vec() {
        let logger = EventLogger::new();
        assert!(logger.flush().is_empty());
    }

    #[test]
    fn concurrent_track_and_flush() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::Arc;
        use std::thread;

        let logger = Arc::new(EventLogger::new());
        let done = Arc::new(AtomicBool::new(false));
        let total_tracked = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..3 {
            let lg = logger.clone();
            let done_cl = done.clone();
            let tracked = total_tracked.clone();
            handles.push(thread::spawn(move || {
                let mut count = 0;
                while !done_cl.load(Ordering::Relaxed) {
                    lg.track(make_event("concurrent"));
                    count += 1;
                }
                tracked.fetch_add(count, Ordering::Relaxed);
            }));
        }

        let lg = logger.clone();
        let mut total_flushed = 0;
        for _ in 0..10 {
            thread::sleep(std::time::Duration::from_millis(10));
            total_flushed += lg.flush().len();
        }
        done.store(true, Ordering::Relaxed);
        for h in handles {
            h.join().unwrap();
        }
        total_flushed += logger.flush().len();

        assert_eq!(total_flushed, total_tracked.load(Ordering::Relaxed));
    }
}
