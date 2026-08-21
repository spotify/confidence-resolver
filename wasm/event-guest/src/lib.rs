use std::sync::LazyLock;

use confidence_event_engine::event_logger::EventLogger;
use confidence_event_engine::proto::confidence::events::v1::{Event, PublishEventsRequest, Void};
use wasm_msg::wasm_msg_guest;
use wasm_msg::WasmResult;

const LOG_TARGET_BYTES: usize = 4 * 1024 * 1024; // 4 MB
const VOID: Void = Void {};

static EVENT_LOGGER: LazyLock<EventLogger> = LazyLock::new(EventLogger::new);

wasm_msg_guest! {
    fn track_event(request: Event) -> WasmResult<Void> {
        EVENT_LOGGER.track(request);
        Ok(VOID)
    }

    fn bounded_flush_events(_request: Void) -> WasmResult<PublishEventsRequest> {
        Ok(EVENT_LOGGER.bounded_flush(LOG_TARGET_BYTES, false))
    }
}
