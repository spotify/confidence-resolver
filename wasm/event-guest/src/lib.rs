use std::sync::LazyLock;

use confidence_event_engine::event_logger::EventLogger;
use confidence_event_engine::proto::confidence::events::v1::Event;
use confidence_event_engine::proto::confidence::events::wasm::v1::{
    FlushEventsResponse, TrackEventRequest, Void,
};
use prost_types::{value::Kind, Struct, Value};
use wasm_msg::wasm_msg_guest;
use wasm_msg::WasmResult;

const LOG_TARGET_BYTES: usize = 2 * 1024 * 1024; // 2 MB
const VOID: Void = Void {};
const EVENT_DEF_PREFIX: &str = "eventDefinitions/";

static EVENT_LOGGER: LazyLock<EventLogger> = LazyLock::new(EventLogger::new);

// Merge order: data fields first, then value and context override.
// If custom data contains keys named "value" or "context", the OpenFeature
// value and evaluation context take precedence (intentional — these are
// reserved keys in the Confidence event payload).
fn build_payload(req: &TrackEventRequest) -> Option<Struct> {
    let mut fields = std::collections::BTreeMap::new();

    if let Some(data) = &req.data {
        for (k, v) in &data.fields {
            fields.insert(k.clone(), v.clone());
        }
    }

    if let Some(value) = req.value {
        fields.insert(
            "value".to_string(),
            Value {
                kind: Some(Kind::NumberValue(value)),
            },
        );
    }

    if let Some(ctx) = &req.context {
        fields.insert(
            "context".to_string(),
            Value {
                kind: Some(Kind::StructValue(ctx.clone())),
            },
        );
    }

    if fields.is_empty() {
        None
    } else {
        Some(Struct { fields })
    }
}

wasm_msg_guest! {
    fn track_event(request: TrackEventRequest) -> WasmResult<Void> {
        let mut event_definition = String::with_capacity(EVENT_DEF_PREFIX.len() + request.event_name.len());
        event_definition.push_str(EVENT_DEF_PREFIX);
        event_definition.push_str(&request.event_name);

        let payload = build_payload(&request);

        EVENT_LOGGER.track(Event {
            event_definition,
            event_time: request.event_time,
            payload,
        });
        Ok(VOID)
    }

    fn bounded_flush_events(_request: Void) -> WasmResult<FlushEventsResponse> {
        Ok(EVENT_LOGGER.bounded_flush(LOG_TARGET_BYTES, false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost_types::{value::Kind, Struct, Value};
    use std::collections::BTreeMap;

    fn str_val(s: &str) -> Value {
        Value {
            kind: Some(Kind::StringValue(s.to_string())),
        }
    }
    fn make_struct(entries: &[(&str, Value)]) -> Struct {
        let mut fields = BTreeMap::new();
        for (k, v) in entries {
            fields.insert(k.to_string(), v.clone());
        }
        Struct { fields }
    }

    #[test]
    fn empty_request_produces_no_payload() {
        let req = TrackEventRequest {
            event_name: "test".into(),
            event_time: None,
            value: None,
            context: None,
            data: None,
        };
        assert!(build_payload(&req).is_none());
    }

    #[test]
    fn data_fields_appear_in_payload() {
        let req = TrackEventRequest {
            event_name: "test".into(),
            event_time: None,
            value: None,
            context: None,
            data: Some(make_struct(&[("key", str_val("val"))])),
        };
        let payload = build_payload(&req).unwrap();
        assert_eq!(
            payload.fields.get("key").and_then(|v| match &v.kind {
                Some(Kind::StringValue(s)) => Some(s.as_str()),
                _ => None,
            }),
            Some("val")
        );
    }

    #[test]
    fn value_overrides_data_collision() {
        let req = TrackEventRequest {
            event_name: "test".into(),
            event_time: None,
            value: Some(42.0),
            context: None,
            data: Some(make_struct(&[("value", str_val("should_be_overridden"))])),
        };
        let payload = build_payload(&req).unwrap();
        assert_eq!(
            payload.fields.get("value").and_then(|v| match &v.kind {
                Some(Kind::NumberValue(n)) => Some(*n),
                _ => None,
            }),
            Some(42.0)
        );
    }

    #[test]
    fn context_overrides_data_collision() {
        let ctx = make_struct(&[("targeting_key", str_val("user-1"))]);
        let req = TrackEventRequest {
            event_name: "test".into(),
            event_time: None,
            value: None,
            context: Some(ctx.clone()),
            data: Some(make_struct(&[("context", str_val("should_be_overridden"))])),
        };
        let payload = build_payload(&req).unwrap();
        match &payload.fields.get("context").unwrap().kind {
            Some(Kind::StructValue(s)) => {
                assert!(s.fields.contains_key("targeting_key"));
            }
            other => panic!("expected struct, got {:?}", other),
        }
    }

    #[test]
    fn value_zero_is_included_when_set() {
        let req = TrackEventRequest {
            event_name: "test".into(),
            event_time: None,
            value: Some(0.0),
            context: None,
            data: None,
        };
        let payload = build_payload(&req).unwrap();
        assert_eq!(
            payload.fields.get("value").and_then(|v| match &v.kind {
                Some(Kind::NumberValue(n)) => Some(*n),
                _ => None,
            }),
            Some(0.0)
        );
    }

    #[test]
    fn all_fields_merge_correctly() {
        let ctx = make_struct(&[("targeting_key", str_val("user-1"))]);
        let data = make_struct(&[("button", str_val("checkout")), ("page", str_val("/cart"))]);
        let req = TrackEventRequest {
            event_name: "test".into(),
            event_time: None,
            value: Some(99.99),
            context: Some(ctx),
            data: Some(data),
        };
        let payload = build_payload(&req).unwrap();
        assert_eq!(payload.fields.len(), 4); // button, page, value, context
        assert!(payload.fields.contains_key("button"));
        assert!(payload.fields.contains_key("page"));
        assert!(payload.fields.contains_key("value"));
        assert!(payload.fields.contains_key("context"));
    }
}
