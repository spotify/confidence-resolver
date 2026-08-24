"""Event engine WASM resolver for Confidence event tracking.

This module provides the EventResolver class that interfaces with the
Confidence event engine WASM module for local event tracking and batching.
"""

import logging

from wasmtime import Config, Engine, Linker, Module, Store
from wasmtime import Trap as WasmTrap

from confidence.proto.confidence.wasm import messages_pb2
from confidence.proto.confidence.events.wasm.v1 import wasm_api_pb2

logger = logging.getLogger(__name__)

# Exception types that indicate a WASM crash requiring reload
WasmCrashError = (RuntimeError, WasmTrap)


class _UnsafeEventWasmResolver:
    """Low-level WASM interface for the event engine.

    Interfaces with the confidence_event_engine.wasm module using the
    wasm-msg protocol. Unlike the flag resolver WASM, the event engine
    has no host imports (no current_time, no log_message).
    """

    def __init__(self, wasm_bytes: bytes) -> None:
        """Initialize the WASM event resolver.

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
        """
        # Pass 0 for unbounded flush
        resp_ptr = self._wasm_msg_guest_bounded_flush_events(self._store, 0)

        data = self._consume(resp_ptr)
        response = messages_pb2.Response()
        response.ParseFromString(data)

        if response.HasField("error") and response.error:
            raise RuntimeError("WASM error: {}".format(response.error))

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
            RuntimeError: If the response contains an error.
        """
        data = self._consume(addr)
        response = messages_pb2.Response()
        response.ParseFromString(data)

        if response.HasField("error") and response.error:
            raise RuntimeError("WASM error: {}".format(response.error))

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


class EventResolver:
    """Event resolver with crash recovery.

    Wraps _UnsafeEventWasmResolver with automatic WASM instance reload
    on RuntimeError or wasmtime.Trap, following the same crash-recovery
    pattern as LocalResolver for the flag resolver.
    """

    def __init__(self, wasm_bytes: bytes) -> None:
        """Initialize the event resolver.

        Args:
            wasm_bytes: The compiled event engine WASM binary bytes.
        """
        self._wasm_bytes = wasm_bytes
        self._delegate = _UnsafeEventWasmResolver(wasm_bytes)

    def track_event(self, request: wasm_api_pb2.TrackEventRequest) -> None:
        """Track an event. On WASM crash, reloads the instance silently.

        Args:
            request: The track event request protobuf.
        """
        try:
            self._delegate.track_event(request)
        except WasmCrashError as error:
            logger.error("Event WASM crashed on track_event, reloading: %s", error)
            self._delegate = _UnsafeEventWasmResolver(self._wasm_bytes)

    def flush_events(self) -> wasm_api_pb2.FlushEventsResponse:
        """Flush pending events. On WASM crash, reloads and returns empty batch.

        Returns:
            A FlushEventsResponse containing the batched events,
            or an empty response on crash.
        """
        try:
            return self._delegate.flush_events()
        except WasmCrashError as error:
            logger.error("Event WASM crashed on flush_events, reloading: %s", error)
            self._delegate = _UnsafeEventWasmResolver(self._wasm_bytes)
            return wasm_api_pb2.FlushEventsResponse()
