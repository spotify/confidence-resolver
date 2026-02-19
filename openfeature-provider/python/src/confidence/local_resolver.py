"""Local resolver with crash recovery.

This module provides the LocalResolver class that wraps WasmResolver
with automatic crash recovery and log buffering.
"""

import logging
from typing import List, Optional, Tuple

from wasmtime import Trap as WasmTrap

from confidence.proto.confidence.wasm import wasm_api_pb2
from confidence.wasm_resolver import WasmResolver

logger = logging.getLogger(__name__)

# Exception types that indicate a WASM crash requiring reload
WasmCrashError = (RuntimeError, WasmTrap)


class LocalResolver:
    """Local flag resolver with crash recovery.

    Wraps WasmResolver and provides:
    - Automatic WASM reload on RuntimeError
    - State caching for recovery
    - Log buffering on crash before reload

    This follows the same pattern as the JavaScript WasmResolver class
    and Go RecoveringResolver.
    """

    def __init__(
        self,
        wasm_bytes: bytes,
        delegate_factory: Optional[type] = None,
    ) -> None:
        """Initialize the local resolver.

        Args:
            wasm_bytes: The compiled WASM binary bytes.
            delegate_factory: Optional factory for creating delegate resolvers.
                Defaults to WasmResolver.
        """
        self._wasm_bytes = wasm_bytes
        self._delegate_factory = delegate_factory or WasmResolver
        self._delegate: WasmResolver = self._delegate_factory(wasm_bytes)
        self._current_state: Optional[Tuple[bytes, str]] = None
        self._buffered_logs: List[bytes] = []

    def _reload_instance(self, error: BaseException) -> None:
        """Reload the WASM instance after a failure.

        Attempts to flush logs from the failing instance before reloading.

        Args:
            error: The error that caused the reload.
        """
        logger.error("Failure calling into wasm: %s", error)

        # Try to flush logs before discarding the old instance
        try:
            logs = self._delegate.flush_logs()
            if logs:
                self._buffered_logs.append(logs)
        except Exception:
            logger.error("Failed to flush_logs on error")

        # Create new delegate
        self._delegate = self._delegate_factory(self._wasm_bytes)

        # Restore state if available
        if self._current_state is not None:
            state, account_id = self._current_state
            try:
                self._delegate.set_resolver_state(state, account_id)
            except Exception as e:
                logger.error("Failed to restore state after reload: %s", e)

    def set_resolver_state(self, state: bytes, account_id: str) -> None:
        """Set the resolver state.

        Stores the state for recovery and delegates to WasmResolver.

        Args:
            state: The serialized resolver state bytes.
            account_id: The account ID for the resolver.
        """
        self._current_state = (state, account_id)
        try:
            self._delegate.set_resolver_state(state, account_id)
        except WasmCrashError as error:
            self._reload_instance(error)
            raise

    def resolve_process(
        self, request: wasm_api_pb2.ResolveProcessRequest
    ) -> wasm_api_pb2.ResolveProcessResponse:
        """Resolve flags using the WASM module.

        On WASM crash (RuntimeError or Trap), reloads the WASM instance and re-raises.

        Args:
            request: The resolve process request.

        Returns:
            The resolve process response.

        Raises:
            RuntimeError: If the WASM module encounters an error.
            WasmTrap: If the WASM module hits a trap (e.g., unreachable).
        """
        try:
            return self._delegate.resolve_process(request)
        except WasmCrashError as error:
            self._reload_instance(error)
            raise

    def flush_logs(self) -> bytes:
        """Flush all pending logs including buffered logs.

        Returns:
            The combined serialized log bytes.
        """
        try:
            current_logs = self._delegate.flush_logs()
            self._buffered_logs.append(current_logs)

            # Combine all buffered logs
            total_length = sum(len(chunk) for chunk in self._buffered_logs)
            result = bytearray(total_length)
            offset = 0
            for chunk in self._buffered_logs:
                result[offset : offset + len(chunk)] = chunk
                offset += len(chunk)

            # Clear buffer
            self._buffered_logs.clear()

            return bytes(result)
        except WasmCrashError as error:
            self._reload_instance(error)
            raise

    def flush_assigned(self) -> bytes:
        """Flush pending assignment logs.

        Returns:
            The serialized assignment data bytes.
        """
        # TODO: Buffer logs and resend on failure (like JS implementation)
        return self._delegate.flush_assigned()
