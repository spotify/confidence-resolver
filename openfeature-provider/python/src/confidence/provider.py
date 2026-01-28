"""Confidence OpenFeature provider implementation.

This module provides the ConfidenceProvider class that implements the OpenFeature
AbstractProvider interface for local flag resolution using the Confidence WASM resolver.
"""

import logging
import threading
from typing import Any, Callable, Dict, List, Optional, Tuple, TypeVar

import grpc
import httpx
from google.protobuf import struct_pb2
from openfeature.evaluation_context import EvaluationContext
from openfeature.event import ProviderEventDetails
from openfeature.exception import ErrorCode
from openfeature.flag_evaluation import FlagResolutionDetails, Reason
from openfeature.provider import AbstractProvider, Metadata, ProviderStatus

from confidence.flag_logger import FlagLogger
from confidence.local_resolver import LocalResolver
from confidence.materialization import (
    InclusionReadOp,
    InclusionReadResult,
    MaterializationNotSupportedError,
    MaterializationStore,
    ReadOp,
    RemoteMaterializationStore,
    UnsupportedMaterializationStore,
    VariantReadOp,
    VariantReadResult,
    VariantWriteOp,
)
from confidence.proto.confidence.flags.resolver.v1 import types_pb2
from confidence.proto.confidence.flags.resolver.v1 import (
    internal_api_pb2,
)
from confidence.proto.confidence.wasm import wasm_api_pb2
from confidence.state_fetcher import StateFetcher
from confidence.version import __version__

# Type variable for generic resolution
T = TypeVar("T")

logger = logging.getLogger(__name__)

# Default intervals
DEFAULT_STATE_POLL_INTERVAL = 30.0
DEFAULT_LOG_POLL_INTERVAL = 10.0
DEFAULT_ASSIGN_POLL_INTERVAL = 0.1


def _load_wasm_from_resources() -> bytes:
    """Load the WASM binary from package resources.

    Returns:
        The WASM binary bytes.

    Raises:
        FileNotFoundError: If the WASM binary cannot be found.
    """
    try:
        # Try importlib.resources first (Python 3.9+)
        import importlib.resources as resources

        try:
            # Python 3.9+ with files()
            files = resources.files("confidence")
            wasm_path = files.joinpath("wasm").joinpath("confidence_resolver.wasm")
            return wasm_path.read_bytes()
        except (AttributeError, FileNotFoundError, TypeError):
            pass

    except ImportError:
        pass

    # Fallback to pkg_resources
    try:
        import pkg_resources

        return pkg_resources.resource_string(
            "confidence", "wasm/confidence_resolver.wasm"
        )
    except Exception:
        pass

    # Development fallback: try resources directory
    from pathlib import Path

    dev_path = (
        Path(__file__).parent.parent.parent
        / "resources"
        / "wasm"
        / "confidence_resolver.wasm"
    )
    if dev_path.exists():
        return dev_path.read_bytes()

    raise FileNotFoundError(
        "Could not find confidence_resolver.wasm in package resources"
    )


class ConfidenceProvider(AbstractProvider):
    """Confidence OpenFeature provider for local flag resolution.

    This provider uses a WASM-based resolver for local flag evaluation,
    with background threads for state polling and log flushing.

    Attributes:
        PROVIDER_NAME: The name of this provider.
    """

    PROVIDER_NAME = "confidence-sdk-python-local"

    def __init__(
        self,
        client_secret: str,
        state_poll_interval: float = DEFAULT_STATE_POLL_INTERVAL,
        log_poll_interval: float = DEFAULT_LOG_POLL_INTERVAL,
        assign_poll_interval: float = DEFAULT_ASSIGN_POLL_INTERVAL,
        materialization_store: Optional[MaterializationStore] = None,
        use_remote_materialization_store: bool = False,
        http_client: Optional[httpx.Client] = None,
        grpc_channel: Optional[grpc.Channel] = None,
        state_fetcher: Optional[StateFetcher] = None,
        flag_logger: Optional[FlagLogger] = None,
        wasm_bytes: Optional[bytes] = None,
    ) -> None:
        """Initialize the Confidence provider.

        Args:
            client_secret: The Confidence client secret (required).
            state_poll_interval: Interval for state polling (default: 30.0).
            log_poll_interval: Interval for log flushing (default: 10.0).
            assign_poll_interval: Interval for assignment flushing (default: 0.1).
            materialization_store: Optional custom materialization store.
            use_remote_materialization_store: Use remote store for materializations.
            http_client: Optional httpx.Client for StateFetcher.
            grpc_channel: Optional grpc.Channel for FlagLogger and MaterializationStore.
            state_fetcher: Optional state fetcher for testing.
            flag_logger: Optional flag logger for testing.
            wasm_bytes: Optional WASM bytes for testing.
        """
        self._client_secret = client_secret
        self._state_poll_interval = state_poll_interval
        self._log_poll_interval = log_poll_interval
        self._assign_poll_interval = assign_poll_interval
        self._http_client = http_client
        self._grpc_channel = grpc_channel

        # Initialize resolver (created during initialize())
        self._resolver: Optional[LocalResolver] = None
        self._resolver_lock = threading.Lock()

        # WASM bytes (loaded lazily or from test)
        self._wasm_bytes = wasm_bytes

        # State fetcher (injected or created)
        self._state_fetcher = state_fetcher

        # Flag logger (injected or created)
        self._flag_logger = flag_logger

        # Materialization store
        if materialization_store is not None:
            self._materialization_store: MaterializationStore = materialization_store
        elif use_remote_materialization_store:
            self._materialization_store = RemoteMaterializationStore(
                client_secret=client_secret,
                channel=grpc_channel,
            )
        else:
            self._materialization_store = UnsupportedMaterializationStore()

        # Background threads
        self._shutdown_event = threading.Event()
        self._state_thread: Optional[threading.Thread] = None
        self._log_thread: Optional[threading.Thread] = None

        # Provider status
        self._status = ProviderStatus.NOT_READY

    def get_status(self) -> ProviderStatus:
        """Get the current provider status.

        Returns:
            The current provider status (NOT_READY, READY, ERROR, STALE, FATAL).
        """
        return self._status

    def get_metadata(self) -> Metadata:
        """Get provider metadata.

        Returns:
            Metadata with the provider name.
        """
        return Metadata(name=self.PROVIDER_NAME)

    def initialize(self, evaluation_context: EvaluationContext) -> None:
        """Initialize the provider.

        Loads the WASM module, fetches initial state, and starts background threads.

        Args:
            evaluation_context: The initial evaluation context.

        Raises:
            Exception: If initialization fails.
        """
        # Load WASM bytes if not provided
        if self._wasm_bytes is None:
            self._wasm_bytes = _load_wasm_from_resources()

        # Create resolver
        self._resolver = LocalResolver(self._wasm_bytes)

        # Create state fetcher if not injected
        if self._state_fetcher is None:
            from confidence.state_fetcher import StateFetcher

            self._state_fetcher = StateFetcher(
                client_secret=self._client_secret,
                http_client=self._http_client,
            )

        # Create flag logger if not injected
        if self._flag_logger is None:
            from confidence.flag_logger import GrpcFlagLogger

            self._flag_logger = GrpcFlagLogger(
                client_secret=self._client_secret,
                channel=self._grpc_channel,
            )

        # Fetch initial state - don't fail if this fails, background thread will retry
        try:
            state, account_id, _ = self._state_fetcher.fetch()
            if account_id:
                self._resolver.set_resolver_state(state, account_id)
                self._status = ProviderStatus.READY
                self.emit_provider_ready(ProviderEventDetails())
                logger.info("ConfidenceProvider initialized successfully")
            else:
                logger.warning(
                    "Initial state load returned empty account ID, "
                    "provider starting in NOT_READY state"
                )
        except Exception as e:
            logger.warning(
                "Initial state load failed, provider starting in NOT_READY state: %s",
                e,
            )

        # Start background threads (will retry state fetch if needed)
        self._start_background_threads()

    def shutdown(self) -> None:
        """Shutdown the provider.

        Stops background threads, flushes logs, and cleans up resources.
        """
        logger.info("Shutting down ConfidenceProvider")

        # Set status to NOT_READY
        self._status = ProviderStatus.NOT_READY

        # Signal shutdown to background threads
        self._shutdown_event.set()

        # Wait for threads to finish
        if self._state_thread is not None:
            self._state_thread.join(timeout=5.0)
            self._state_thread = None

        if self._log_thread is not None:
            self._log_thread.join(timeout=5.0)
            self._log_thread = None

        # Flush final logs
        if self._resolver is not None:
            try:
                log_data = self._resolver.flush_logs()
                if log_data and self._flag_logger is not None:
                    self._flag_logger.write(log_data)
            except Exception as e:
                logger.error("Failed to flush final logs: %s", e)

        # Shutdown flag logger
        if self._flag_logger is not None:
            self._flag_logger.shutdown()

        logger.info("ConfidenceProvider shutdown complete")

    def _resolve_typed(
        self,
        flag_key: str,
        default_value: T,
        evaluation_context: Optional[EvaluationContext],
        type_check: Callable[[Any], bool],
        type_convert: Callable[[Any], T],
    ) -> FlagResolutionDetails[T]:
        """Generic typed resolution with type checking and conversion."""
        result = self._resolve_object(flag_key, default_value, evaluation_context)

        if result.value is None or result.error_code is not None:
            return FlagResolutionDetails(
                value=default_value,
                reason=result.reason,
                error_code=result.error_code,
                error_message=result.error_message,
                variant=result.variant,
            )

        if not type_check(result.value):
            return FlagResolutionDetails(
                value=default_value,
                reason=Reason.ERROR,
                error_code=ErrorCode.TYPE_MISMATCH,
                error_message=f"Value is not {type(default_value).__name__}",
            )

        return FlagResolutionDetails(
            value=type_convert(result.value),
            reason=result.reason,
            variant=result.variant,
        )

    def resolve_boolean_details(
        self,
        flag_key: str,
        default_value: bool,
        evaluation_context: Optional[EvaluationContext] = None,
    ) -> FlagResolutionDetails[bool]:
        """Resolve a boolean flag."""
        return self._resolve_typed(
            flag_key,
            default_value,
            evaluation_context,
            type_check=lambda v: isinstance(v, bool),
            type_convert=lambda v: v,
        )

    def resolve_string_details(
        self,
        flag_key: str,
        default_value: str,
        evaluation_context: Optional[EvaluationContext] = None,
    ) -> FlagResolutionDetails[str]:
        """Resolve a string flag."""
        return self._resolve_typed(
            flag_key,
            default_value,
            evaluation_context,
            type_check=lambda v: isinstance(v, str),
            type_convert=lambda v: v,
        )

    def resolve_integer_details(
        self,
        flag_key: str,
        default_value: int,
        evaluation_context: Optional[EvaluationContext] = None,
    ) -> FlagResolutionDetails[int]:
        """Resolve an integer flag."""
        return self._resolve_typed(
            flag_key,
            default_value,
            evaluation_context,
            # Accept int (but not bool) or float
            type_check=lambda v: (isinstance(v, int) and not isinstance(v, bool))
            or isinstance(v, float),
            type_convert=lambda v: int(v),
        )

    def resolve_float_details(
        self,
        flag_key: str,
        default_value: float,
        evaluation_context: Optional[EvaluationContext] = None,
    ) -> FlagResolutionDetails[float]:
        """Resolve a float flag."""
        return self._resolve_typed(
            flag_key,
            default_value,
            evaluation_context,
            # Accept int or float (but not bool)
            type_check=lambda v: isinstance(v, (int, float))
            and not isinstance(v, bool),
            type_convert=lambda v: float(v),
        )

    def resolve_object_details(
        self,
        flag_key: str,
        default_value: Dict[str, Any],
        evaluation_context: Optional[EvaluationContext] = None,
    ) -> FlagResolutionDetails[Dict[str, Any]]:
        """Resolve an object flag."""
        return self._resolve_typed(
            flag_key,
            default_value,
            evaluation_context,
            type_check=lambda v: isinstance(v, dict),
            type_convert=lambda v: v,
        )

    def _resolve_object(
        self,
        flag_key: str,
        default_value: Any,
        evaluation_context: Optional[EvaluationContext],
    ) -> FlagResolutionDetails[Any]:
        """Core resolution logic for all flag types.

        Args:
            flag_key: The flag key (may include path).
            default_value: The default value.
            evaluation_context: The evaluation context.

        Returns:
            The resolved flag details.
        """
        # Check resolver is ready
        if self._resolver is None:
            return FlagResolutionDetails(
                value=default_value,
                reason=Reason.ERROR,
                error_code=ErrorCode.PROVIDER_NOT_READY,
                error_message="Provider not initialized",
            )

        try:
            # Parse flag path
            flag_name, path = self._parse_flag_path(flag_key)

            # Convert evaluation context to proto
            proto_context = self._context_to_proto(evaluation_context)

            # Build resolve request
            request = wasm_api_pb2.ResolveWithStickyRequest()
            request.resolve_request.flags.append(f"flags/{flag_name}")
            request.resolve_request.client_secret = self._client_secret
            request.resolve_request.apply = True
            if proto_context is not None:
                request.resolve_request.evaluation_context.CopyFrom(proto_context)

            # Set SDK info
            request.resolve_request.sdk.id = types_pb2.SdkId.SDK_ID_PYTHON_PROVIDER
            request.resolve_request.sdk.version = __version__

            # Resolve with materialization handling
            response = self._resolve_with_materialization(request)

            # Check for resolved flags
            if not response.HasField("success"):
                return FlagResolutionDetails(
                    value=default_value,
                    reason=Reason.ERROR,
                    error_code=ErrorCode.GENERAL,
                    error_message="Missing materializations",
                )

            resolved_flags = response.success.response.resolved_flags
            if len(resolved_flags) == 0:
                return FlagResolutionDetails(
                    value=default_value,
                    reason=Reason.ERROR,
                    error_code=ErrorCode.FLAG_NOT_FOUND,
                    error_message=f"Flag '{flag_name}' not found",
                )

            resolved_flag = resolved_flags[0]

            # Verify flag name matches
            expected_name = f"flags/{flag_name}"
            if resolved_flag.flag != expected_name:
                return FlagResolutionDetails(
                    value=default_value,
                    reason=Reason.ERROR,
                    error_code=ErrorCode.FLAG_NOT_FOUND,
                    error_message="Unexpected flag returned",
                )

            # Check if variant is assigned
            if not resolved_flag.variant:
                return FlagResolutionDetails(
                    value=default_value,
                    reason=self._map_resolve_reason(resolved_flag.reason),
                )

            # Convert protobuf struct to Python dict
            value = self._proto_struct_to_dict(resolved_flag.value)

            # Extract nested path if specified
            if path:
                value, found = self._get_value_for_path(path, value)
                if not found:
                    return FlagResolutionDetails(
                        value=default_value,
                        reason=Reason.ERROR,
                        error_code=ErrorCode.FLAG_NOT_FOUND,
                        error_message=f"Path '{path}' not found in flag '{flag_name}'",
                    )

            return FlagResolutionDetails(
                value=value,
                reason=self._map_resolve_reason(resolved_flag.reason),
                variant=resolved_flag.variant,
            )

        except Exception as e:
            logger.error("Failed to resolve flag '%s': %s", flag_key, e)
            return FlagResolutionDetails(
                value=default_value,
                reason=Reason.ERROR,
                error_code=ErrorCode.GENERAL,
                error_message=str(e),
            )

    def _resolve_with_materialization(
        self, request: wasm_api_pb2.ResolveWithStickyRequest
    ) -> wasm_api_pb2.ResolveWithStickyResponse:
        """Resolve with materialization handling.

        Args:
            request: The resolve request.

        Returns:
            The resolve response.

        Raises:
            RuntimeError: If resolution fails.
        """
        with self._resolver_lock:
            response = self._resolver.resolve_with_sticky(request)

        # Check if materializations are needed
        if response.HasField("read_ops_request"):
            # Read materializations
            try:
                read_results = self._read_materializations(response.read_ops_request)
            except MaterializationNotSupportedError as e:
                raise RuntimeError(
                    f"failed to handle missing materializations: {e.message}"
                )

            # Retry with materializations
            request.materializations.extend(read_results)
            with self._resolver_lock:
                response = self._resolver.resolve_with_sticky(request)

        # Handle materialization writes
        if response.HasField("success"):
            updates = response.success.materialization_updates
            if updates:
                self._write_materializations(updates)

        return response

    def _read_materializations(
        self, read_request: internal_api_pb2.ReadOperationsRequest
    ) -> List[internal_api_pb2.ReadResult]:
        """Read materializations from the store.

        Args:
            read_request: The read request.

        Returns:
            List of read results.
        """
        ops: List[ReadOp] = []
        for op in read_request.ops:
            if op.HasField("variant_read_op"):
                vop = op.variant_read_op
                ops.append(
                    VariantReadOp(
                        unit=vop.unit,
                        materialization=vop.materialization,
                        rule=vop.rule,
                    )
                )
            elif op.HasField("inclusion_read_op"):
                iop = op.inclusion_read_op
                ops.append(
                    InclusionReadOp(
                        unit=iop.unit,
                        materialization=iop.materialization,
                    )
                )

        results = self._materialization_store.read(ops)

        # Convert to proto format
        proto_results = []
        for result in results:
            proto_result = internal_api_pb2.ReadResult()
            if isinstance(result, VariantReadResult):
                proto_result.variant_result.unit = result.unit
                proto_result.variant_result.materialization = result.materialization
                proto_result.variant_result.rule = result.rule
                if result.variant is not None:
                    proto_result.variant_result.variant = result.variant
            elif isinstance(result, InclusionReadResult):
                proto_result.inclusion_result.unit = result.unit
                proto_result.inclusion_result.materialization = result.materialization
                proto_result.inclusion_result.is_included = result.included
            proto_results.append(proto_result)

        return proto_results

    def _write_materializations(
        self, updates: List[internal_api_pb2.VariantData]
    ) -> None:
        """Write materializations to the store.

        Args:
            updates: The materialization updates to write.
        """
        try:
            ops = []
            for update in updates:
                ops.append(
                    VariantWriteOp(
                        unit=update.unit,
                        materialization=update.materialization,
                        rule=update.rule,
                        variant=update.variant,
                    )
                )
            self._materialization_store.write(ops)
        except MaterializationNotSupportedError:
            logger.warning("Materialization write not supported")
        except Exception as e:
            logger.error("Failed to write materializations: %s", e)

    def _flush_assigned(self) -> None:
        """Flush assigned logs."""
        if self._resolver is None or self._flag_logger is None:
            return

        try:
            with self._resolver_lock:
                log_data = self._resolver.flush_assigned()
            if log_data:
                self._flag_logger.write(log_data)
        except Exception as e:
            logger.error("Failed to flush assigned logs: %s", e)

    def _start_background_threads(self) -> None:
        """Start background threads for state polling and log flushing."""
        self._shutdown_event.clear()

        # State polling thread
        self._state_thread = threading.Thread(
            target=self._state_poll_loop,
            daemon=True,
            name="confidence-state-poll",
        )
        self._state_thread.start()

        # Log flush thread
        self._log_thread = threading.Thread(
            target=self._log_flush_loop,
            daemon=True,
            name="confidence-log-flush",
        )
        self._log_thread.start()

    def _state_poll_loop(self) -> None:
        """Background loop for state polling."""
        # Use shorter retry interval when NOT_READY
        retry_interval = 1.0

        while True:
            # Use short interval if NOT_READY, normal interval otherwise
            interval = (
                retry_interval
                if self._status == ProviderStatus.NOT_READY
                else self._state_poll_interval
            )

            if self._shutdown_event.wait(timeout=interval):
                break

            try:
                state, account_id, changed = self._state_fetcher.fetch()
                if changed and account_id:
                    with self._resolver_lock:
                        self._resolver.set_resolver_state(state, account_id)
                    logger.debug("Resolver state updated")

                # If we were NOT_READY and now have valid state, transition to READY
                if account_id and self._status == ProviderStatus.NOT_READY:
                    self._status = ProviderStatus.READY
                    self.emit_provider_ready(ProviderEventDetails())
                    logger.info("Provider recovered and is now READY")
            except Exception as e:
                logger.error("State fetch failed: %s", e)

    def _log_flush_loop(self) -> None:
        """Background loop for log flushing."""
        last_full_flush = 0.0
        last_assign_flush = 0.0

        while not self._shutdown_event.is_set():
            import time

            now = time.time()

            # Full flush at log_poll_interval
            if now - last_full_flush >= self._log_poll_interval:
                try:
                    with self._resolver_lock:
                        log_data = self._resolver.flush_logs()
                    if log_data and self._flag_logger is not None:
                        self._flag_logger.write(log_data)
                except Exception as e:
                    logger.error("Failed to flush logs: %s", e)
                last_full_flush = now

            # Assign flush at assign_poll_interval
            if now - last_assign_flush >= self._assign_poll_interval:
                self._flush_assigned()
                last_assign_flush = now

            # Sleep for shortest interval
            self._shutdown_event.wait(timeout=min(self._assign_poll_interval, 0.1))

    @staticmethod
    def _parse_flag_path(flag_key: str) -> Tuple[str, str]:
        """Parse a flag key into flag name and path.

        Args:
            flag_key: The flag key (e.g., "my-flag.nested.value").

        Returns:
            Tuple of (flag_name, path) where path may be empty.
        """
        parts = flag_key.split(".", 1)
        if len(parts) == 1:
            return parts[0], ""
        return parts[0], parts[1]

    @staticmethod
    def _context_to_proto(
        context: Optional[EvaluationContext],
    ) -> Optional[struct_pb2.Struct]:
        """Convert EvaluationContext to protobuf Struct.

        Args:
            context: The evaluation context.

        Returns:
            The protobuf Struct, or None if context is None.
        """
        if context is None:
            return None

        fields: Dict[str, struct_pb2.Value] = {}

        # Add targeting key as targeting_key
        if context.targeting_key:
            fields["targeting_key"] = struct_pb2.Value(
                string_value=context.targeting_key
            )

        # Add attributes
        if context.attributes:
            for key, value in context.attributes.items():
                fields[key] = ConfidenceProvider._value_to_proto(value)

        return struct_pb2.Struct(fields=fields)

    @staticmethod
    def _value_to_proto(value: Any) -> struct_pb2.Value:
        """Convert a Python value to protobuf Value.

        Args:
            value: The Python value.

        Returns:
            The protobuf Value.
        """
        if value is None:
            return struct_pb2.Value(null_value=struct_pb2.NullValue.NULL_VALUE)
        elif isinstance(value, bool):
            return struct_pb2.Value(bool_value=value)
        elif isinstance(value, (int, float)):
            return struct_pb2.Value(number_value=float(value))
        elif isinstance(value, str):
            return struct_pb2.Value(string_value=value)
        elif isinstance(value, list):
            list_value = struct_pb2.ListValue(
                values=[ConfidenceProvider._value_to_proto(v) for v in value]
            )
            return struct_pb2.Value(list_value=list_value)
        elif isinstance(value, dict):
            struct_value = struct_pb2.Struct(
                fields={
                    k: ConfidenceProvider._value_to_proto(v) for k, v in value.items()
                }
            )
            return struct_pb2.Value(struct_value=struct_value)
        else:
            return struct_pb2.Value(string_value=str(value))

    @staticmethod
    def _proto_struct_to_dict(struct: struct_pb2.Struct) -> Dict[str, Any]:
        """Convert protobuf Struct to Python dict.

        Args:
            struct: The protobuf Struct.

        Returns:
            The Python dict.
        """
        if struct is None:
            return {}

        result: Dict[str, Any] = {}
        for key, value in struct.fields.items():
            result[key] = ConfidenceProvider._proto_value_to_python(value)
        return result

    @staticmethod
    def _proto_value_to_python(value: struct_pb2.Value) -> Any:
        """Convert protobuf Value to Python value.

        Args:
            value: The protobuf Value.

        Returns:
            The Python value.
        """
        kind = value.WhichOneof("kind")
        if kind == "null_value":
            return None
        elif kind == "bool_value":
            return value.bool_value
        elif kind == "number_value":
            return value.number_value
        elif kind == "string_value":
            return value.string_value
        elif kind == "list_value":
            return [
                ConfidenceProvider._proto_value_to_python(v)
                for v in value.list_value.values
            ]
        elif kind == "struct_value":
            return ConfidenceProvider._proto_struct_to_dict(value.struct_value)
        else:
            return None

    @staticmethod
    def _get_value_for_path(path: str, value: Any) -> Tuple[Any, bool]:
        """Extract a nested value using dot notation.

        Args:
            path: The dot-separated path (e.g., "nested.value").
            value: The value to extract from.

        Returns:
            Tuple of (extracted_value, found).
        """
        if not path:
            return value, True

        parts = path.split(".")
        current = value

        for part in parts:
            if isinstance(current, dict) and part in current:
                current = current[part]
            else:
                return None, False

        return current, True

    @staticmethod
    def _map_resolve_reason(reason: types_pb2.ResolveReason) -> Reason:
        """Map protobuf ResolveReason to OpenFeature Reason.

        Args:
            reason: The protobuf ResolveReason.

        Returns:
            The OpenFeature Reason.
        """
        if reason == types_pb2.ResolveReason.RESOLVE_REASON_MATCH:
            return Reason.TARGETING_MATCH
        elif reason == types_pb2.ResolveReason.RESOLVE_REASON_NO_SEGMENT_MATCH:
            return Reason.DEFAULT
        elif reason == types_pb2.ResolveReason.RESOLVE_REASON_FLAG_ARCHIVED:
            return Reason.DISABLED
        elif reason in (
            types_pb2.ResolveReason.RESOLVE_REASON_TARGETING_KEY_ERROR,
            types_pb2.ResolveReason.RESOLVE_REASON_ERROR,
        ):
            return Reason.ERROR
        else:
            return Reason.UNKNOWN
