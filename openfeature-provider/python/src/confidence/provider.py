"""Confidence OpenFeature provider implementation.

This module provides the ConfidenceProvider class that implements the OpenFeature
AbstractProvider interface for local flag resolution using the Confidence WASM resolver.
"""

import json
import logging
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
from typing import Any, Callable, Dict, List, Optional, Tuple, TypeVar

import grpc
import httpx
from google.protobuf import struct_pb2
from google.protobuf.timestamp_pb2 import Timestamp
from openfeature.evaluation_context import EvaluationContext
from openfeature.event import ProviderEventDetails
from openfeature.exception import ErrorCode
from openfeature.flag_evaluation import FlagResolutionDetails, Reason
from openfeature.provider import AbstractProvider, Metadata, ProviderStatus

from confidence.event_resolver import EventResolver
from confidence.flag_logger import (
    FlagLogger,
    MultiDestinationFlagLogger,
)
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
from confidence.proto.confidence.events.v1 import api_pb2 as events_api_pb2
from confidence.proto.confidence.events.v1 import api_pb2_grpc as events_api_pb2_grpc
from confidence.proto.confidence.events.v1 import types_pb2 as events_types_pb2
from confidence.proto.confidence.events.wasm.v1 import wasm_api_pb2 as events_wasm_pb2
from confidence.proto.confidence.flags.resolver.v1 import (
    api_pb2,
    internal_api_pb2,
    types_pb2,
)
from confidence.proto.confidence.wasm import wasm_api_pb2
from confidence.state_fetcher import StateFetcher
from confidence.version import __version__

# Type variable for generic resolution
T = TypeVar("T")

logger = logging.getLogger(__name__)

# Default intervals
DEFAULT_STATE_POLL_INTERVAL = 30.0
DEFAULT_LOG_POLL_INTERVAL = 15.0
DEFAULT_ASSIGN_POLL_INTERVAL = 0.1

# gRPC target for the Confidence events service
EVENTS_GRPC_TARGET = "edge-grpc.spotify.com:443"

# Timeout in seconds for a single PublishEvents RPC
EVENTS_PUBLISH_TIMEOUT = 30.0

# Number of PublishEvents attempts between failure-rate log lines. Publish
# failures are swallowed per batch, so this window is the only signal that
# events are being dropped. Mirrors the flag logger's stats window.
EVENTS_STATS_WINDOW = 10

# A single WASM flush is capped (2 MB), so draining a backlog needs several
# flushes. Bounded because _send_events swallows network failures: an unbounded
# loop would spin forever if the events API is unreachable during shutdown.
MAX_EVENT_DRAIN_BATCHES = 100

# Retry transient UNAVAILABLE failures when publishing events. Scoped to the
# events service so it cannot affect any other RPC on the channel.
_EVENTS_RETRY_SERVICE_CONFIG = json.dumps(
    {
        "methodConfig": [
            {
                "name": [{"service": "confidence.events.v1.EventsService"}],
                "retryPolicy": {
                    "maxAttempts": 3,
                    "initialBackoff": "1s",
                    "maxBackoff": "10s",
                    "backoffMultiplier": 2.0,
                    "retryableStatusCodes": ["UNAVAILABLE"],
                },
            }
        ]
    }
)


class SnapshotConfig:
    """Configuration for :meth:`ConfidenceProvider.get_prometheus_metrics`.

    **Experimental:** this API is subject to change.
    """

    pass


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
        encryption_key: Optional[str] = None,
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
        event_wasm_path: Optional[str] = None,
        event_wasm_bytes: Optional[bytes] = None,
        enable_apply_dedup: bool = False,
        disable_exposure_collection: bool = False,
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
            event_wasm_path: Optional file path to confidence_event_engine.wasm.
                When provided, enables event tracking via track().
            event_wasm_bytes: Optional event engine WASM bytes (for testing).
                When provided, enables event tracking via track().
            enable_apply_dedup: Experimental — enable apply-event dedup in
                the WASM resolver: repeated identical assignments within a
                short TTL window are logged once. Off by default; the API may
                change.
            disable_exposure_collection: Disable exposure/assignment collection for all
                OpenFeature evaluations through this provider. Use only for
                exceptional no-exposure modes; resolve logs and telemetry are
                still sent.
        """
        self._client_secret = client_secret
        self._encryption_key = encryption_key
        self._init_labels: Dict[str, str] = {
            "encryption": str(bool(encryption_key)).lower()
        }
        self._init_telemetry_state = "pending"
        self._init_telemetry_lock = threading.Lock()
        self._state_poll_interval = state_poll_interval
        self._log_poll_interval = log_poll_interval
        self._assign_poll_interval = assign_poll_interval
        self._http_client = http_client
        self._grpc_channel = grpc_channel
        self._enable_apply_dedup = enable_apply_dedup
        self._disable_exposure_collection = disable_exposure_collection

        # Initialize resolver (created during initialize())
        self._resolver: Optional[LocalResolver] = None
        self._resolver_lock = threading.Lock()

        # WASM bytes (loaded lazily or from test)
        self._wasm_bytes = wasm_bytes

        # Event engine configuration
        self._event_wasm_path = event_wasm_path
        self._event_wasm_bytes = event_wasm_bytes
        self._event_resolver: Optional[EventResolver] = None
        self._event_resolver_lock = threading.Lock()
        self._event_executor = ThreadPoolExecutor(max_workers=2)
        self._events_channel: Optional[grpc.Channel] = None
        self._events_stub: Optional[events_api_pb2_grpc.EventsServiceStub] = None
        self._event_stats_lock = threading.Lock()
        self._event_publish_attempts = 0
        self._event_publish_failures = 0

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
        if not self._encryption_key:
            logger.warning(
                "No encryption_key provided. Falling back to unencrypted state. "
                "An encryption key will be required in an upcoming version."
            )

        # Load WASM bytes if not provided
        if self._wasm_bytes is None:
            self._wasm_bytes = _load_wasm_from_resources()

        # Create resolver
        self._resolver = LocalResolver(self._wasm_bytes)

        # Initialize event resolver if configured
        event_bytes = self._event_wasm_bytes
        if event_bytes is None and self._event_wasm_path is not None:
            try:
                with open(self._event_wasm_path, "rb") as f:
                    event_bytes = f.read()
            except Exception as e:
                logger.error(
                    "Failed to load event engine WASM from %s: %s",
                    self._event_wasm_path,
                    e,
                )

        if event_bytes is not None:
            try:
                self._event_resolver = EventResolver(event_bytes)
                self._events_channel = grpc.secure_channel(
                    EVENTS_GRPC_TARGET,
                    grpc.ssl_channel_credentials(),
                    options=[("grpc.service_config", _EVENTS_RETRY_SERVICE_CONFIG)],
                )
                self._events_stub = events_api_pb2_grpc.EventsServiceStub(
                    self._events_channel
                )
                logger.info("Event tracking enabled")
            except Exception as e:
                logger.error("Failed to initialize event resolver: %s", e)

        # Create state fetcher if not injected
        if self._state_fetcher is None:
            from confidence.state_fetcher import StateFetcher

            self._state_fetcher = StateFetcher(
                client_secret=self._client_secret,
                http_client=self._http_client,
                encryption_key=self._encryption_key,
            )

        # Fetch initial state - don't fail if this fails, background thread will retry
        try:
            state, account_id, _, log_destinations = self._state_fetcher.fetch()

            # Create flag logger if not injected, using destinations from CDN
            if self._flag_logger is None:
                self._flag_logger = self._create_flag_logger(
                    account_id, log_destinations
                )

            if account_id:
                sdk = types_pb2.Sdk(
                    id=types_pb2.SdkId.SDK_ID_PYTHON_PROVIDER,
                    version=__version__,
                )
                self._resolver.set_resolver_state(
                    state,
                    account_id,
                    sdk,
                    self._enable_apply_dedup,
                    self._disable_exposure_collection,
                )
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
            # Create a default flag logger if fetch failed and none was injected
            if self._flag_logger is None:
                self._flag_logger = self._create_flag_logger("", [])

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
                self._write_logs(self._resolver.flush_logs())
            except Exception as e:
                logger.error("Failed to flush final logs: %s", e)

        # Drain pending events. A single flush is capped inside the WASM, so
        # anything beyond that cap needs further flushes or it is dropped.
        if self._event_resolver is not None:
            try:
                self._drain_events()
            except Exception as e:
                logger.error("Failed to flush final events: %s", e)

        # Shutdown event executor and gRPC channel
        self._event_executor.shutdown(wait=True)
        if self._events_channel is not None:
            self._events_channel.close()
            self._events_channel = None
            self._events_stub = None

        # Shutdown flag logger
        if self._flag_logger is not None:
            self._flag_logger.shutdown()

        # Close materialization store if it exposes a close method
        close_store = getattr(self._materialization_store, "close", None)
        if callable(close_store):
            try:
                close_store()
            except Exception as e:
                logger.error("Failed to close materialization store: %s", e)

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
        start_time = time.monotonic()
        result = self._resolve_object(
            flag_key, default_value, evaluation_context, start_time
        )

        if result.value is None or result.error_code is not None:
            return FlagResolutionDetails(
                value=default_value,
                reason=result.reason,
                error_code=result.error_code,
                error_message=result.error_message,
                variant=result.variant,
            )

        if not type_check(result.value):
            self._do_register_resolve(
                types_pb2.RESOLVE_REASON_TYPE_MISMATCH, start_time
            )
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
            # Accept int (but not bool) or float that is a whole number
            type_check=lambda v: (
                (isinstance(v, int) and not isinstance(v, bool))
                or (isinstance(v, float) and v.is_integer())
            ),
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
            type_check=lambda v: (
                isinstance(v, (int, float)) and not isinstance(v, bool)
            ),
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
        start_time: float,
    ) -> FlagResolutionDetails[Any]:
        """Core resolution logic for all flag types."""

        if self._resolver is None:
            return FlagResolutionDetails(
                value=default_value,
                reason=Reason.ERROR,
                error_code=ErrorCode.PROVIDER_NOT_READY,
                error_message="Provider not initialized",
            )

        try:
            flag_name, path = self._parse_flag_path(flag_key)

            disable_exposure_collection = self._disable_exposure_collection
            if evaluation_context and evaluation_context.attributes:
                disable_exposure_collection = disable_exposure_collection or (
                    evaluation_context.attributes.get("_confidence_skip_apply", False)
                    is True
                )

            proto_context = self._context_to_proto(evaluation_context)

            resolve_req = api_pb2.ResolveFlagsRequest()
            resolve_req.flags.append(f"flags/{flag_name}")
            resolve_req.client_secret = self._client_secret
            # apply=False covers both provider disable_exposure_collection and per-eval
            # `_confidence_skip_apply`. Provider disable_exposure_collection is also set on the
            # WASM guest via set_resolver_state so assign/token are skipped
            # entirely; apply=False alone would still mint a deferred token.
            resolve_req.apply = not disable_exposure_collection
            if proto_context is not None:
                resolve_req.evaluation_context.CopyFrom(proto_context)
            resolve_req.sdk.id = types_pb2.SdkId.SDK_ID_PYTHON_PROVIDER
            resolve_req.sdk.version = __version__

            request = wasm_api_pb2.ResolveProcessRequest()
            if self._materialization_store is not None and not isinstance(
                self._materialization_store, UnsupportedMaterializationStore
            ):
                request.deferred_materializations.CopyFrom(resolve_req)
            else:
                request.without_materializations.CopyFrom(resolve_req)

            response = self._resolve_with_materialization(request)

            if not response.HasField("resolved"):
                return FlagResolutionDetails(
                    value=default_value,
                    reason=Reason.ERROR,
                    error_code=ErrorCode.GENERAL,
                    error_message="Unexpected suspended response",
                )

            resolved_flags = response.resolved.response.resolved_flags
            if len(resolved_flags) == 0:
                self._do_register_resolve(
                    types_pb2.RESOLVE_REASON_FLAG_NOT_FOUND, start_time
                )
                return FlagResolutionDetails(
                    value=default_value,
                    reason=Reason.ERROR,
                    error_code=ErrorCode.FLAG_NOT_FOUND,
                    error_message=f"Flag '{flag_name}' not found",
                )

            resolved_flag = resolved_flags[0]

            expected_name = f"flags/{flag_name}"
            if resolved_flag.flag != expected_name:
                self._do_register_resolve(
                    types_pb2.RESOLVE_REASON_FLAG_NOT_FOUND, start_time
                )
                return FlagResolutionDetails(
                    value=default_value,
                    reason=Reason.ERROR,
                    error_code=ErrorCode.FLAG_NOT_FOUND,
                    error_message="Unexpected flag returned",
                )

            if (
                resolved_flag.reason
                == types_pb2.ResolveReason.RESOLVE_REASON_MATERIALIZATION_NOT_SUPPORTED
            ):
                logger.warning(
                    "Flag '%s' requires materializations but no materialization store is "
                    "configured. Enable it via ConfidenceProvider(use_remote_materialization_store=True)",
                    flag_name,
                )
                self._do_register_resolve(resolved_flag.reason, start_time)
                return FlagResolutionDetails(
                    value=default_value,
                    reason=Reason.ERROR,
                    error_code=ErrorCode.GENERAL,
                    error_message=f"Flag '{flag_name}' requires materializations. Configure a materialization store.",
                )

            if not resolved_flag.variant:
                self._do_register_resolve(resolved_flag.reason, start_time)
                return FlagResolutionDetails(
                    value=default_value,
                    reason=self._map_resolve_reason(resolved_flag.reason),
                )

            value = self._proto_struct_to_dict(resolved_flag.value)

            if path:
                value, found = self._get_value_for_path(path, value)
                if not found:
                    self._do_register_resolve(
                        types_pb2.RESOLVE_REASON_FLAG_NOT_FOUND, start_time
                    )
                    return FlagResolutionDetails(
                        value=default_value,
                        reason=Reason.ERROR,
                        error_code=ErrorCode.FLAG_NOT_FOUND,
                        error_message=f"Path '{path}' not found in flag '{flag_name}'",
                    )

            self._do_register_resolve(resolved_flag.reason, start_time)
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

    def _do_register_resolve(self, reason: int, start_time: float) -> None:
        """Register a resolve evaluation for telemetry."""
        if self._resolver is None:
            return
        latency_us = int((time.monotonic() - start_time) * 1_000_000)
        try:
            request = wasm_api_pb2.RegisterResolveRequest()
            request.reason = reason
            request.latency_us = min(latency_us, 2**32 - 1)
            with self._resolver_lock:
                self._resolver.register_resolve(request)
        except Exception:
            logger.warning("Failed to register resolve telemetry", exc_info=True)

    def _resolve_with_materialization(
        self, request: wasm_api_pb2.ResolveProcessRequest
    ) -> wasm_api_pb2.ResolveProcessResponse:
        """Resolve with materialization handling (suspend/resume).

        Args:
            request: The resolve process request.

        Returns:
            The resolve process response.

        Raises:
            RuntimeError: If resolution fails.
        """
        with self._resolver_lock:
            response = self._resolver.resolve_process(request)

        return self._handle_response(response)

    def _handle_response(
        self, response: wasm_api_pb2.ResolveProcessResponse
    ) -> wasm_api_pb2.ResolveProcessResponse:
        """Handle initial resolve response."""
        if response.HasField("resolved"):
            if response.resolved.materializations_to_write:
                self._write_materializations(
                    response.resolved.materializations_to_write
                )
            return response

        if response.HasField("suspended"):
            # Read materializations from store
            try:
                materializations = self._read_materializations(
                    response.suspended.materializations_to_read
                )
            except MaterializationNotSupportedError as e:
                raise RuntimeError(f"failed to read materializations: {e.message}")

            # Build resume request
            resume_request = wasm_api_pb2.ResolveProcessRequest()
            resume_request.resume.state = response.suspended.state
            for mat in materializations:
                resume_request.resume.materializations.append(mat)

            with self._resolver_lock:
                resume_response = self._resolver.resolve_process(resume_request)

            return self._handle_resume_response(resume_response)

        raise RuntimeError("Unexpected empty resolve response")

    def _handle_resume_response(
        self, response: wasm_api_pb2.ResolveProcessResponse
    ) -> wasm_api_pb2.ResolveProcessResponse:
        """Handle response after resume - should not suspend again."""
        if response.HasField("resolved"):
            if response.resolved.materializations_to_write:
                self._write_materializations(
                    response.resolved.materializations_to_write
                )
            return response

        if response.HasField("suspended"):
            raise RuntimeError("Unexpected second suspend after resume")

        raise RuntimeError("Unexpected empty resolve response after resume")

    def _read_materializations(
        self, records: List[wasm_api_pb2.MaterializationRecord]
    ) -> List[wasm_api_pb2.MaterializationRecord]:
        """Read materializations from the store.

        Converts MaterializationRecords to ReadOps, queries the store,
        then converts results back to MaterializationRecords for a Resume request.

        Args:
            records: The MaterializationRecords from a Suspended response.

        Returns:
            List of MaterializationRecords for the Resume request.
        """
        ops: List[ReadOp] = []
        for record in records:
            if record.rule:
                ops.append(
                    VariantReadOp(
                        unit=record.unit,
                        materialization=record.materialization,
                        rule=record.rule,
                    )
                )
            else:
                ops.append(
                    InclusionReadOp(
                        unit=record.unit,
                        materialization=record.materialization,
                    )
                )

        results = self._materialization_store.read(ops)

        # Convert results back to MaterializationRecords.
        # Records with empty variant and "not included" inclusion results are omitted
        # (absence = no prior assignment / not included).
        materialization_records = []
        for result in results:
            if isinstance(result, VariantReadResult):
                if result.variant:  # Only include if variant is non-empty
                    rec = wasm_api_pb2.MaterializationRecord()
                    rec.unit = result.unit
                    rec.materialization = result.materialization
                    rec.rule = result.rule
                    rec.variant = result.variant
                    materialization_records.append(rec)
                # No prior assignment → omit (absence = no sticky assignment)
            elif isinstance(result, InclusionReadResult):
                if result.included:
                    rec = wasm_api_pb2.MaterializationRecord()
                    rec.unit = result.unit
                    rec.materialization = result.materialization
                    materialization_records.append(rec)
                # Not included → omit (absence = not included)

        return materialization_records

    def _write_materializations(
        self, records: List[wasm_api_pb2.MaterializationRecord]
    ) -> None:
        """Write materializations to the store.

        Args:
            records: The MaterializationRecords from a Resolved response.
        """
        try:
            ops = []
            for record in records:
                ops.append(
                    VariantWriteOp(
                        unit=record.unit,
                        materialization=record.materialization,
                        rule=record.rule,
                        variant=record.variant,
                    )
                )
            self._materialization_store.write(ops)
        except MaterializationNotSupportedError:
            logger.warning("Materialization write not supported")
        except Exception as e:
            logger.error("Failed to write materializations: %s", e)

    def _write_logs(self, log_data: bytes) -> None:
        if not log_data or self._flag_logger is None:
            return

        include_init = False
        with self._init_telemetry_lock:
            if self._init_telemetry_state == "pending":
                self._init_telemetry_state = "sending"
                include_init = True

        if include_init:
            request = internal_api_pb2.WriteFlagLogsRequest.FromString(log_data)
            request.telemetry_data.sdk.CopyFrom(
                types_pb2.Sdk(
                    id=types_pb2.SdkId.SDK_ID_PYTHON_PROVIDER,
                    version=__version__,
                )
            )
            init_rate = request.telemetry_data.provider_init_rate.add()
            init_rate.count = 1
            for k, v in self._init_labels.items():
                init_rate.labels[k] = v
            log_data = request.SerializeToString()

        try:
            self._flag_logger.write(log_data)
        except Exception:
            if include_init:
                with self._init_telemetry_lock:
                    self._init_telemetry_state = "pending"
            raise
        else:
            if include_init:
                with self._init_telemetry_lock:
                    self._init_telemetry_state = "sent"

    def _create_flag_logger(
        self, account_id: str, log_destinations: List[int]
    ) -> FlagLogger:
        """Create a flag logger based on the given log destinations.

        If destinations are provided, creates a MultiDestinationFlagLogger.
        Otherwise, falls back to a GrpcFlagLogger (Spotify Edge).

        Args:
            account_id: The account ID (needed for Cloudflare destination).
            log_destinations: Ordered list of LogDestination enum values.

        Returns:
            A FlagLogger instance.
        """
        if log_destinations:
            return MultiDestinationFlagLogger(
                client_secret=self._client_secret,
                account_id=account_id,
                log_destinations=log_destinations,
                grpc_channel=self._grpc_channel,
            )

        # Default: gRPC to Spotify Edge
        from confidence.flag_logger import GrpcFlagLogger

        return GrpcFlagLogger(
            client_secret=self._client_secret,
            channel=self._grpc_channel,
        )

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

    def track(
        self,
        event_name: str,
        context: Optional[EvaluationContext] = None,
        value: Optional[float] = None,
        data: Optional[Dict[str, Any]] = None,
    ) -> None:
        """Track an event for the Confidence events API.

        Requires the provider to be initialized with event_wasm_path or
        event_wasm_bytes. If event tracking is not configured, this method
        is a no-op.

        Args:
            event_name: The bare event name (e.g. "my_event").
            context: Optional OpenFeature evaluation context.
            value: Optional numeric value associated with the event.
            data: Optional custom data dictionary for the event.
        """
        if self._event_resolver is None:
            return

        try:
            request = events_wasm_pb2.TrackEventRequest()
            request.event_name = event_name

            # Set event_time to now
            now = datetime.now(timezone.utc)
            timestamp = Timestamp()
            timestamp.FromDatetime(now)
            request.event_time.CopyFrom(timestamp)

            # Set optional value
            if value is not None:
                request.value = value

            # Convert context to proto Struct
            proto_context = self._context_to_proto(context)
            if proto_context is not None:
                request.context.CopyFrom(proto_context)

            # Convert data to proto Struct
            if data:
                data_struct = struct_pb2.Struct(
                    fields={k: self._value_to_proto(v) for k, v in data.items()}
                )
                request.data.CopyFrom(data_struct)

            with self._event_resolver_lock:
                self._event_resolver.track_event(request)
        except Exception:
            logger.warning("Failed to track event '%s'", event_name, exc_info=True)

    def _flush_events(self) -> int:
        """Flush pending events from the event resolver and send them.

        Returns:
            The number of events handed off for publishing. A single flush is
            capped inside the WASM engine, so a non-zero result does not mean
            the buffer is now empty.
        """
        if self._event_resolver is None:
            return 0

        with self._event_resolver_lock:
            batch = self._event_resolver.flush_events()

        if not batch.events:
            return 0

        self._event_executor.submit(self._send_events, batch)
        return len(batch.events)

    def _drain_events(self) -> None:
        """Flush events repeatedly until the event buffer is empty.

        A single flush is capped at 2 MB inside the WASM engine, so one flush
        can leave a backlog behind. Bounded to MAX_EVENT_DRAIN_BATCHES because
        _send_events swallows network failures.
        """
        if self._event_resolver is None:
            return

        for _ in range(MAX_EVENT_DRAIN_BATCHES):
            if self._flush_events() == 0:
                return

        logger.warning(
            "Event drain hit the %d-batch limit on shutdown; dropping the rest",
            MAX_EVENT_DRAIN_BATCHES,
        )

    def _send_events(self, batch: events_wasm_pb2.FlushEventsResponse) -> None:
        """Publish a batch of events to the Confidence events service over gRPC.

        Runs in the event executor thread pool.

        Args:
            batch: The FlushEventsResponse from the WASM flush.
        """
        if self._events_stub is None:
            return

        failed = False
        try:
            send_time = Timestamp()
            send_time.FromDatetime(datetime.now(timezone.utc))
            request = events_api_pb2.PublishEventsRequest(
                client_secret=self._client_secret,
                events=batch.events,
                send_time=send_time,
                sdk=events_types_pb2.Sdk(
                    id=events_types_pb2.SDK_ID_PYTHON_LOCAL_PROVIDER,
                    version=__version__,
                ),
            )
            response = self._events_stub.PublishEvents(
                request, timeout=EVENTS_PUBLISH_TIMEOUT
            )
            for error in response.errors:
                logger.error(
                    "Failed to publish event at index %d: %s %s",
                    error.index,
                    events_types_pb2.EventError.Reason.Name(error.reason),
                    error.message,
                )
        except Exception:
            failed = True
            logger.warning("Failed to send events", exc_info=True)

        with self._event_stats_lock:
            if failed:
                self._event_publish_failures += 1
            self._event_publish_attempts += 1
            if self._event_publish_attempts % EVENTS_STATS_WINDOW == 0:
                if self._event_publish_failures > 0:
                    logger.warning(
                        "Event publish failures: %d/%d",
                        self._event_publish_failures,
                        EVENTS_STATS_WINDOW,
                    )
                self._event_publish_failures = 0

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
                (
                    state,
                    account_id,
                    changed,
                    log_destinations,
                ) = self._state_fetcher.fetch()
                if changed and account_id:
                    sdk = types_pb2.Sdk(
                        id=types_pb2.SdkId.SDK_ID_PYTHON_PROVIDER,
                        version=__version__,
                    )
                    # Flush logs before state update to reduce WASM heap fragmentation (#455)
                    with self._resolver_lock:
                        flushed_logs = self._resolver.flush_logs()
                        self._resolver.set_resolver_state(
                            state,
                            account_id,
                            sdk,
                            self._enable_apply_dedup,
                            self._disable_exposure_collection,
                        )
                    self._write_logs(flushed_logs)

                    # Update account ID and destinations on the logger
                    if self._flag_logger is not None:
                        set_account = getattr(self._flag_logger, "set_account_id", None)
                        if callable(set_account):
                            set_account(account_id)
                        update_dests = getattr(
                            self._flag_logger, "update_destinations", None
                        )
                        if callable(update_dests):
                            update_dests(log_destinations)

                    logger.debug("Resolver state updated")

                # If we were NOT_READY and now have valid state, transition to READY
                if account_id and self._status == ProviderStatus.NOT_READY:
                    self._status = ProviderStatus.READY
                    self.emit_provider_ready(ProviderEventDetails())
                    logger.info("Provider recovered and is now READY")
            except Exception as e:
                logger.error("State fetch failed: %s", e)

    def _log_flush_loop(self) -> None:
        """Background loop for log flushing and event flushing."""
        last_full_flush = 0.0
        last_assign_flush = 0.0
        last_event_flush = 0.0

        while not self._shutdown_event.is_set():
            import time

            now = time.time()

            # Full flush at log_poll_interval
            if now - last_full_flush >= self._log_poll_interval:
                try:
                    with self._resolver_lock:
                        log_data = self._resolver.flush_logs()
                    self._write_logs(log_data)
                except Exception as e:
                    logger.error("Failed to flush logs: %s", e)
                last_full_flush = now

            # Event flush at log_poll_interval (same cadence as log flush)
            if now - last_event_flush >= self._log_poll_interval:
                if self._event_resolver is not None:
                    try:
                        self._flush_events()
                    except Exception as e:
                        logger.error("Failed to flush events: %s", e)
                last_event_flush = now

            # Assign flush at assign_poll_interval (skipped when disable_exposure_collection)
            if (
                not self._disable_exposure_collection
                and now - last_assign_flush >= self._assign_poll_interval
            ):
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

        # Add attributes, skipping the internal apply-control flag
        if context.attributes:
            for key, value in context.attributes.items():
                if key == "_confidence_skip_apply":
                    continue
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

    def get_prometheus_metrics(self, request: Optional["SnapshotConfig"] = None) -> str:
        """Get a Prometheus metrics snapshot.

        **Experimental:** this API is subject to change.

        Args:
            request: Optional config (reserved for future options).

        Returns:
            The Prometheus metrics text, or empty string if not initialized.
        """
        if self._resolver is None:
            return ""
        with self._resolver_lock:
            return self._resolver.prometheus_snapshot()

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
            types_pb2.ResolveReason.RESOLVE_REASON_UNRECOGNIZED_TARGETING_RULE,
            types_pb2.ResolveReason.RESOLVE_REASON_MATERIALIZATION_NOT_SUPPORTED,
        ):
            return Reason.ERROR
        else:
            return Reason.UNKNOWN
