"""WASM-based flag resolver using wasmtime.

This module provides the WasmResolver class that interfaces with the
Confidence flag resolver WASM module for local flag resolution.
"""

from datetime import datetime
from typing import Optional

from google.protobuf import message as protobuf_message
from google.protobuf.timestamp_pb2 import Timestamp
from wasmtime import Config, Engine, Func, FuncType, Linker, Module, Store, ValType

from confidence_openfeature.proto.confidence.wasm import messages_pb2
from confidence_openfeature.proto.confidence.wasm import wasm_api_pb2


class WasmResolver:
    """WASM-based flag resolver.

    Interfaces with the Confidence WASM resolver module for local flag resolution.
    Uses wasmtime for WASM execution.

    Memory protocol: Data is exchanged with WASM using a 4-byte little-endian
    length prefix stored at ptr-4. The length includes the 4 prefix bytes.
    """

    def __init__(self, wasm_bytes: bytes) -> None:
        """Initialize the WASM resolver.

        Args:
            wasm_bytes: The compiled WASM binary bytes.
        """
        # Create WASM engine and store
        config = Config()
        self._engine = Engine(config)
        self._store = Store(self._engine)

        # Compile the WASM module
        self._module = Module(self._engine, wasm_bytes)

        # Register host functions and instantiate
        self._register_host_functions()

        # Get exported functions
        exports = self._instance.exports(self._store)
        self._wasm_msg_alloc = exports["wasm_msg_alloc"]
        self._wasm_msg_free = exports["wasm_msg_free"]
        self._wasm_msg_guest_set_resolver_state = exports[
            "wasm_msg_guest_set_resolver_state"
        ]
        self._wasm_msg_guest_resolve_with_sticky = exports[
            "wasm_msg_guest_resolve_with_sticky"
        ]
        self._wasm_msg_guest_bounded_flush_logs = exports[
            "wasm_msg_guest_bounded_flush_logs"
        ]
        self._wasm_msg_guest_bounded_flush_assign = exports[
            "wasm_msg_guest_bounded_flush_assign"
        ]
        self._memory = exports["memory"]

    def _register_host_functions(self) -> None:
        """Register host functions that can be called from WASM."""

        def current_time(ptr: int) -> int:
            """Host function to return current timestamp."""
            try:
                # Create timestamp from current time
                now = datetime.now()
                timestamp = Timestamp()
                timestamp.FromDatetime(now)

                # Create response wrapper with timestamp data
                response = messages_pb2.Response()
                response.data = timestamp.SerializeToString()

                # Transfer response to WASM memory
                return self._transfer(response.SerializeToString())
            except Exception as e:
                # Return error response
                error_response = messages_pb2.Response()
                error_response.error = str(e)
                return self._transfer(error_response.SerializeToString())

        # Create function type: takes one i32 parameter, returns one i32
        func_type = FuncType([ValType.i32()], [ValType.i32()])
        host_func_time = Func(self._store, func_type, current_time)

        linker = Linker(self._store.engine)

        # Define the import with module and name
        linker.define(
            self._store, "wasm_msg", "wasm_msg_host_current_time", host_func_time
        )

        # Instantiate the module with imports
        self._instance = linker.instantiate(self._store, self._module)

    def set_resolver_state(self, state: bytes, account_id: str) -> None:
        """Set the resolver state in the WASM module.

        Args:
            state: The serialized resolver state bytes.
            account_id: The account ID for the resolver.
        """
        request = messages_pb2.SetResolverStateRequest()
        request.state = state
        request.account_id = account_id

        req_ptr = self._transfer_request(request)
        resp_ptr = self._wasm_msg_guest_set_resolver_state(self._store, req_ptr)

        if resp_ptr != 0:
            self._consume_response(resp_ptr, None)

    def resolve_with_sticky(
        self, request: wasm_api_pb2.ResolveWithStickyRequest
    ) -> wasm_api_pb2.ResolveWithStickyResponse:
        """Resolve flags using the WASM module.

        Args:
            request: The resolve request with flags and evaluation context.

        Returns:
            The resolve response with resolved flag values.
        """
        req_ptr = self._transfer_request(request)
        resp_ptr = self._wasm_msg_guest_resolve_with_sticky(self._store, req_ptr)

        response = wasm_api_pb2.ResolveWithStickyResponse()
        self._consume_response(resp_ptr, response)
        return response

    def flush_logs(self) -> bytes:
        """Flush all pending logs from the WASM module.

        Returns:
            The serialized WriteFlagLogsRequest bytes.
        """
        # Pass 0 for unbounded flush
        resp_ptr = self._wasm_msg_guest_bounded_flush_logs(self._store, 0)

        # Read raw response data
        data = self._consume(resp_ptr)
        response = messages_pb2.Response()
        response.ParseFromString(data)

        if response.HasField("error") and response.error:
            raise RuntimeError(f"WASM error: {response.error}")

        return response.data if response.data else b""

    def flush_assigned(self) -> bytes:
        """Flush pending assignment logs from the WASM module.

        Returns:
            The serialized assignment data bytes.
        """
        # Pass 0 for unbounded flush
        resp_ptr = self._wasm_msg_guest_bounded_flush_assign(self._store, 0)

        # Read raw response data
        data = self._consume(resp_ptr)
        response = messages_pb2.Response()
        response.ParseFromString(data)

        if response.HasField("error") and response.error:
            raise RuntimeError(f"WASM error: {response.error}")

        return response.data if response.data else b""

    def _transfer_request(self, message: protobuf_message.Message) -> int:
        """Transfer a protobuf message to WASM memory as a Request.

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
        # Allocate memory in WASM
        ptr = self._wasm_msg_alloc(self._store, len(data))

        # Write data to WASM memory
        self._memory.write(self._store, data, ptr)

        return ptr

    def _consume_response(
        self, addr: int, target: Optional[protobuf_message.Message]
    ) -> None:
        """Consume a response from WASM memory.

        Args:
            addr: The address in WASM memory.
            target: Optional protobuf message to parse the data into.

        Raises:
            RuntimeError: If the response contains an error.
        """
        data = self._consume(addr)
        response = messages_pb2.Response()
        response.ParseFromString(data)

        if response.HasField("error") and response.error:
            raise RuntimeError(f"WASM error: {response.error}")

        if target is not None and response.data:
            target.ParseFromString(response.data)

    def _consume(self, addr: int) -> bytes:
        """Read data from WASM memory and free it.

        Memory protocol: 4-byte little-endian length prefix at addr-4.
        The length value includes the 4 prefix bytes.

        Args:
            addr: The address in WASM memory.

        Returns:
            The bytes read from memory.
        """
        # Read length (4-byte little-endian prefix at addr-4)
        len_bytes = self._memory.read(self._store, addr - 4, addr)
        total_len = int.from_bytes(len_bytes, byteorder="little")
        length = total_len - 4

        # Read data
        data = self._memory.read(self._store, addr, addr + length)

        # Make a copy before freeing (defensive copy)
        data_copy = bytes(data)

        # Free memory
        self._wasm_msg_free(self._store, addr)

        return data_copy
