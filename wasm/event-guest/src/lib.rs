use std::sync::LazyLock;

use confidence_event_engine::event_logger::EventLogger;
use confidence_event_engine::proto::confidence::events::v1::{
    PublishEvent, PublishEventsRequest, TrackEventRequest, Void,
};
use prost_types::{value::Kind, Struct, Value};
use wasm_msg::wasm_msg_guest;
use wasm_msg::WasmResult;

const LOG_TARGET_BYTES: usize = 2 * 1024 * 1024; // 2 MB
const VOID: Void = Void {};
const EVENT_DEF_PREFIX: &str = "eventDefinitions/";

static EVENT_LOGGER: LazyLock<EventLogger> = LazyLock::new(EventLogger::new);

fn build_payload(req: &TrackEventRequest) -> Option<Struct> {
    let mut fields = std::collections::BTreeMap::new();

    if let Some(data) = &req.data {
        for (k, v) in &data.fields {
            fields.insert(k.clone(), v.clone());
        }
    }

    if req.value != 0.0 {
        fields.insert(
            "value".to_string(),
            Value {
                kind: Some(Kind::NumberValue(req.value)),
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

        EVENT_LOGGER.track(PublishEvent {
            event_definition,
            event_time: request.event_time,
            payload,
        });
        Ok(VOID)
    }

    fn bounded_flush_events(_request: Void) -> WasmResult<PublishEventsRequest> {
        Ok(EVENT_LOGGER.bounded_flush(LOG_TARGET_BYTES, false))
    }
}
