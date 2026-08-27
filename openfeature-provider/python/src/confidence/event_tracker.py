"""Event engine WASM tracker for Confidence event tracking.

Provides the EventTracker class that interfaces with the Confidence event
engine WASM module for local event tracking and batching. Named to match the
Go provider's event_tracking package: it tracks events, it does not resolve
anything.
"""

import logging

from wasmtime import Config, Engine, Linker, Module, Store
from wasmtime import Trap as WasmTrap
from wasmtime import WasmtimeError

from confidence.proto.confidence.events.wasm.v1 import wasm_api_pb2
from confidence.proto.confidence.wasm import messages_pb2

logger = logging.getLogger(__name__)

# Faults that can leave the WASM instance in an undefined state, so the instance
# must be rebuilt. Deliberately narrow: reloading discards every event buffered
# inside the instance, so it must not be triggered by errors that leave the
# engine healthy. Mirrors errWasmFatal in the Go event tracker.
WasmCrashError = (WasmTrap, WasmtimeError)


class EventEngineError(RuntimeError):
    """An error the guest reported cleanly through the Response envelope.

    The WASM instance is still healthy, so this must NOT trigger a reload —
    that would throw away the instance's buffered events for nothing. Subclasses
    RuntimeError so existing callers catching RuntimeError still work.
    """


class _UnsafeEventWasmTracker:
    """Low-level WASM interface for the event engine.

    Interfaces with the confidence_event_engine.wasm module using the
    wasm-msg protocol. Unlike the flag resolver WASM, the event engine
    has no host imports (no current_time, no log_message).
    """

    def __init__(self, wasm_bytes: bytes) -> None:
        """Initialize the WASM event tracker.

        Args:
            wasm_bytes: The compiled event engine WASM binary bytes.
        """
        config = Config()
        config.cache = True
        self._engine = Engine(config)
        self._store = Store(self._engine)
        self._module = Module(self._engine, wasm_bytes)

        # No host imports needed for the event engine
        linker = Linker(self._engine)
        self._instance = linker.instantiate(self._store, self._module)

        # Get exported functions
        exports = self._instance.exports(self._store)
        self._wasm_msg_alloc = exports["wasm_msg_alloc"]
        self._wasm_msg_free = exports["wasm_msg_free"]
        self._wasm_msg_guest_track_event = exports["wasm_msg_guest_track_event"]
        self._wasm_msg_guest_bounded_flush_events = exports[
            "wasm_msg_guest_bounded_flush_events"
        ]
        self._memory = exports["memory"]

    def track_event(self, request: wasm_api_pb2.TrackEventRequest) -> None:
        """Track an event by sending it to the WASM event engine.

        Args:
            request: The track event request protobuf.
        """
        req_ptr = self._transfer_request(request)
        resp_ptr = self._wasm_msg_guest_track_event(self._store, req_ptr)
        if resp_ptr != 0:
            self._consume_response(resp_ptr)

    def flush_events(self) -> wasm_api_pb2.FlushEventsResponse:
        """Flush all pending events from the WASM event engine.

        Returns:
            A FlushEventsResponse containing the batched events.

        Raises:
            EventEngineError: If the guest reported an error.
        """
        resp_ptr = self._wasm_msg_guest_bounded_flush_events(self._store, 0)
        if resp_ptr == 0:
            # No response to consume. Falling through would make _consume read
            # the length prefix at addr-4, i.e. a wrapped-around address.
            return wasm_api_pb2.FlushEventsResponse()

        data = self._consume(resp_ptr)
        response = messages_pb2.Response()
        response.ParseFromString(data)

        if response.HasField("error") and response.error:
            raise EventEngineError("WASM error: {}".format(response.error))

        result = wasm_api_pb2.FlushEventsResponse()
        if response.data:
            result.ParseFromString(response.data)
        return result

    def _transfer_request(self, message: wasm_api_pb2.TrackEventRequest) -> int:
        """Transfer a protobuf message to WASM memory as a Request envelope.

        Args:
            message: The protobuf message to transfer.

        Returns:
            The pointer to the data in WASM memory.
        """
        data = message.SerializeToString()
        request = messages_pb2.Request()
        request.data = data
        return self._transfer(request.SerializeToString())

    def _transfer(self, data: bytes) -> int:
        """Allocate memory in WASM and copy data.

        Args:
            data: The bytes to copy to WASM memory.

        Returns:
            The pointer to the data in WASM memory.
        """
        ptr = self._wasm_msg_alloc(self._store, len(data))
        self._memory.write(self._store, data, ptr)
        return ptr

    def _consume_response(self, addr: int) -> None:
        """Consume a wasm-msg Response envelope and check for errors.

        Args:
            addr: The address in WASM memory.

        Raises:
            EventEngineError: If the response contains an error.
        """
        data = self._consume(addr)
        response = messages_pb2.Response()
        response.ParseFromString(data)

        if response.HasField("error") and response.error:
            raise EventEngineError("WASM error: {}".format(response.error))

    def _consume(self, addr: int) -> bytes:
        """Read data from WASM memory and free it.

        Memory protocol: 4-byte little-endian length prefix at addr-4.
        The length value includes the 4 prefix bytes.

        Args:
            addr: The address in WASM memory.

        Returns:
            The bytes read from memory.
        """
        len_bytes = self._memory.read(self._store, addr - 4, addr)
        total_len = int.from_bytes(len_bytes, byteorder="little")
        length = total_len - 4

        data = self._memory.read(self._store, addr, addr + length)
        data_copy = bytes(data)

        self._wasm_msg_free(self._store, addr)
        return data_copy


class EventTracker:
    """Event tracker with crash recovery.

    Wraps _UnsafeEventWasmTracker and rebuilds the WASM instance on a genuine
    WASM fault, following the same crash-recovery pattern LocalResolver uses for
    the flag resolver.

    A reload discards every event buffered inside the instance, so only faults
    in WasmCrashError trigger one. Errors the guest reported cleanly
    (EventEngineError) and protobuf failures leave the instance healthy and are
    propagated to the caller instead.
    """

    def __init__(self, wasm_bytes: bytes) -> None:
        """Initialize the event tracker.

        Args:
            wasm_bytes: The compiled event engine WASM binary bytes.
        """
        self._wasm_bytes = wasm_bytes
        self._delegate = _UnsafeEventWasmTracker(wasm_bytes)

    def track_event(self, request: wasm_api_pb2.TrackEventRequest) -> None:
        """Track an event. On a WASM fault, reloads the instance.

        Args:
            request: The track event request protobuf.

        Raises:
            EventEngineError: If the guest reported an error. The instance is
                healthy and its buffered events are preserved.
        """
        try:
            self._delegate.track_event(request)
        except WasmCrashError as error:
            logger.error("Event WASM crashed on track_event, reloading: %s", error)
            self._delegate = _UnsafeEventWasmTracker(self._wasm_bytes)

    def flush_events(self) -> wasm_api_pb2.FlushEventsResponse:
        """Flush pending events. On a WASM fault, reloads and returns empty.

        Returns:
            A FlushEventsResponse containing the batched events, or an empty
            response if the instance faulted and was reloaded.

        Raises:
            EventEngineError: If the guest reported an error. The instance is
                healthy and its buffered events are preserved.
        """
        try:
            return self._delegate.flush_events()
        except WasmCrashError as error:
            logger.error("Event WASM crashed on flush_events, reloading: %s", error)
            self._delegate = _UnsafeEventWasmTracker(self._wasm_bytes)
            return wasm_api_pb2.FlushEventsResponse()
