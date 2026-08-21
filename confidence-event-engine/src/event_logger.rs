use std::collections::VecDeque;
use std::sync::Mutex;

use crate::proto::confidence::events::v1::{PublishEvent, PublishEventsRequest};
use prost::{length_delimiter_len, Message};

#[derive(Debug, Default)]
struct State {
    pending: VecDeque<(PublishEvent, usize)>,
    pending_bytes: usize,
}

#[derive(Debug, Default)]
pub struct EventLogger {
    queue: crossbeam_queue::SegQueue<PublishEvent>,
    state: Mutex<State>,
}

impl EventLogger {
    pub fn new() -> Self {
        Self {
            ..Default::default()
        }
    }

    pub fn track(&self, event: PublishEvent) {
        self.queue.push(event);
    }

    pub fn bounded_flush(&self, limit_bytes: usize, require_full: bool) -> PublishEventsRequest {
        let mut req = PublishEventsRequest::default();
        self.flush_fill(&mut req, limit_bytes, require_full);
        req
    }

    pub fn flush_fill(
        &self,
        req: &mut PublishEventsRequest,
        limit_bytes: usize,
        require_full: bool,
    ) -> usize {
        let mut state = match self.state.lock() {
            Ok(g) => g,
            Err(err) => err.into_inner(),
        };
        let start = req.encoded_len();
        let limit_bytes = limit_bytes.saturating_sub(start);

        while state.pending_bytes < limit_bytes {
            if let Some(event) = self.queue.pop() {
                let len = Self::encoded_len(&event);
                state.pending.push_back((event, len));
                state.pending_bytes = state.pending_bytes.saturating_add(len);
            } else {
                break;
            }
        }

        let mut written: usize = 0;
        if state.pending_bytes >= limit_bytes || !require_full {
            while let Some((_, len)) = state.pending.front() {
                if written.saturating_add(*len) <= limit_bytes || written == 0 && start == 0 {
                    written = written.saturating_add(*len);
                    let event = unsafe { state.pending.pop_front().unwrap_unchecked().0 };
                    req.events.push(event);
                } else {
                    break;
                }
            }
            state.pending_bytes = state.pending_bytes.saturating_sub(written);
        }
        written
    }

    fn encoded_len(event: &PublishEvent) -> usize {
        let len = event.encoded_len();
        len.saturating_add(length_delimiter_len(len))
            .saturating_add(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event() -> PublishEvent {
        PublishEvent {
            event_definition: "eventDefinitions/test_event".to_string(),
            event_time: Some(prost_types::Timestamp {
                seconds: 1000,
                nanos: 0,
            }),
            payload: None,
        }
    }

    fn make_event_with_payload() -> PublishEvent {
        use prost_types::{value::Kind, Struct, Value};
        let mut fields = std::collections::BTreeMap::new();
        fields.insert(
            "key".to_string(),
            Value {
                kind: Some(Kind::StringValue("value".to_string())),
            },
        );
        PublishEvent {
            event_definition: "eventDefinitions/rich_event".to_string(),
            event_time: Some(prost_types::Timestamp {
                seconds: 2000,
                nanos: 500_000_000,
            }),
            payload: Some(Struct { fields }),
        }
    }

    #[test]
    fn event_size_is_correctly_calculated() {
        let ev = make_event();
        let ev_size = EventLogger::encoded_len(&ev);
        let req = PublishEventsRequest {
            events: vec![ev.clone(), ev],
        };
        assert_eq!(2 * ev_size, req.encoded_len());
    }

    #[test]
    fn flush_returns_all_events_when_under_limit() {
        let logger = EventLogger::new();
        logger.track(make_event());
        logger.track(make_event_with_payload());
        let req = logger.bounded_flush(10_000, false);
        assert_eq!(req.events.len(), 2);
    }

    #[test]
    fn flush_respects_byte_limit() {
        let ev_size = EventLogger::encoded_len(&make_event());
        let logger = EventLogger::new();
        logger.track(make_event());
        logger.track(make_event());
        logger.track(make_event());
        let req = logger.bounded_flush(3 * ev_size - 1, true);
        assert_eq!(req.events.len(), 2);
    }

    #[test]
    fn first_event_exceeding_limit_is_sent_alone() {
        let logger = EventLogger::new();
        logger.track(make_event());
        logger.track(make_event());
        let req = logger.bounded_flush(1, true);
        assert_eq!(req.events.len(), 1);
    }

    #[test]
    fn require_full_returns_empty_when_under_target() {
        let logger = EventLogger::new();
        let req = logger.bounded_flush(10_000, true);
        assert!(req.events.is_empty());
    }

    #[test]
    fn pending_events_survive_across_flushes() {
        let ev_size = EventLogger::encoded_len(&make_event());
        let logger = EventLogger::new();
        logger.track(make_event());
        logger.track(make_event());
        logger.track(make_event());
        let req1 = logger.bounded_flush(2 * ev_size, false);
        assert_eq!(req1.events.len(), 2);
        let req2 = logger.bounded_flush(10_000, false);
        assert_eq!(req2.events.len(), 1);
    }

    #[test]
    fn empty_flush_returns_no_events() {
        let logger = EventLogger::new();
        let req = logger.bounded_flush(10_000, false);
        assert!(req.events.is_empty());
    }

    #[test]
    fn events_preserve_data() {
        let logger = EventLogger::new();
        logger.track(make_event_with_payload());
        let req = logger.bounded_flush(10_000, false);
        assert_eq!(req.events.len(), 1);
        assert_eq!(
            req.events[0].event_definition,
            "eventDefinitions/rich_event"
        );
        assert_eq!(req.events[0].event_time.as_ref().unwrap().seconds, 2000);
        assert!(req.events[0].payload.is_some());
    }
}
