"""Flag logger implementations for Confidence OpenFeature provider.

This module provides flag logging functionality to send flag assignment events
to the Confidence backend via gRPC.
"""

import json
import logging
import threading
from concurrent.futures import ThreadPoolExecutor
from typing import Optional, Protocol, runtime_checkable

import grpc

from confidence.proto.confidence.flags.resolver.v1 import (
    internal_api_pb2,
    internal_api_pb2_grpc,
)

logger = logging.getLogger(__name__)

# gRPC target for the Confidence edge service
GRPC_TARGET = "edge-grpc.spotify.com:443"

_RETRY_SERVICE_CONFIG = json.dumps(
    {
        "methodConfig": [
            {
                "name": [
                    {
                        "service": "confidence.flags.resolver.v1.InternalFlagLoggerService"
                    }
                ],
                "retryPolicy": {
                    "maxAttempts": 3,
                    "initialBackoff": "0.5s",
                    "maxBackoff": "5s",
                    "backoffMultiplier": 2.0,
                    "retryableStatusCodes": ["UNAVAILABLE"],
                },
            }
        ]
    }
)


@runtime_checkable
class FlagLogger(Protocol):
    """Protocol for flag logging."""

    def write(self, request_bytes: bytes) -> None:
        """Write flag logs asynchronously.

        Args:
            request_bytes: Serialized WriteFlagLogsRequest proto bytes.
        """
        ...

    def shutdown(self) -> None:
        """Shutdown the logger and wait for pending writes to complete."""
        ...


class GrpcFlagLogger:
    """gRPC-based flag logger that sends flag logs to the Confidence backend.

    Writes are performed asynchronously using a thread pool. The logger
    skips empty requests (no flag_assigned, client_resolve_info, or flag_resolve_info).
    """

    def __init__(
        self,
        client_secret: str,
        channel: Optional[grpc.Channel] = None,
    ) -> None:
        """Initialize the gRPC flag logger.

        Args:
            client_secret: The Confidence client secret for authentication.
            channel: Optional gRPC channel for testing. If not provided,
                    a secure channel to GRPC_TARGET will be created.
        """
        self._client_secret = client_secret
        self._executor = ThreadPoolExecutor(max_workers=2)
        self._stats_lock = threading.Lock()
        self._attempts = 0
        self._failures = 0

        if channel is not None:
            self._channel = channel
            self._owns_channel = False
        else:
            self._channel = grpc.secure_channel(
                GRPC_TARGET,
                grpc.ssl_channel_credentials(),
                options=[("grpc.service_config", _RETRY_SERVICE_CONFIG)],
            )
            self._owns_channel = True

        self._stub = internal_api_pb2_grpc.InternalFlagLoggerServiceStub(self._channel)

    def write(self, request_bytes: bytes) -> None:
        """Write flag logs asynchronously.

        Skips empty requests (no data).

        Args:
            request_bytes: Serialized WriteFlagLogsRequest proto bytes.
        """
        # Skip empty bytes
        if not request_bytes:
            logger.debug("Skipping empty flag log request (empty bytes)")
            return

        # Parse the request to check if it has any data
        try:
            request = internal_api_pb2.WriteFlagLogsRequest()
            request.ParseFromString(request_bytes)
        except Exception as e:
            logger.error("Failed to parse WriteFlagLogsRequest: %s", e)
            return

        # Skip if all lists are empty
        if (
            len(request.flag_assigned) == 0
            and len(request.client_resolve_info) == 0
            and len(request.flag_resolve_info) == 0
        ):
            logger.debug("Skipping empty flag log request (no data)")
            return

        # Submit async write
        self._executor.submit(self._send_request, request)

    def _send_request(self, request: internal_api_pb2.WriteFlagLogsRequest) -> None:
        """Send the request to the backend (runs in thread pool).

        Args:
            request: The WriteFlagLogsRequest to send.
        """
        failed = False
        try:
            metadata = [("authorization", f"ClientSecret {self._client_secret}")]
            self._stub.ClientWriteFlagLogs(request, metadata=metadata, timeout=30.0)
            logger.debug(
                "Successfully sent flag log with %d entries",
                len(request.flag_assigned),
            )
        except Exception:
            failed = True

        with self._stats_lock:
            if failed:
                self._failures += 1
            self._attempts += 1
            if self._attempts % 10 == 0:
                if self._failures > 0:
                    logger.warning(
                        "Flag log write failures: %d/10", self._failures
                    )
                self._failures = 0

    def shutdown(self) -> None:
        """Shutdown the logger and wait for pending writes to complete."""
        self._executor.shutdown(wait=True)
        if self._owns_channel:
            self._channel.close()


class NoOpFlagLogger:
    """A no-op flag logger that drops all requests.

    Useful for testing or when flag logging should be disabled.
    """

    def write(self, request_bytes: bytes) -> None:
        """Drop the request (do nothing).

        Args:
            request_bytes: Ignored.
        """
        pass

    def shutdown(self) -> None:
        """Do nothing (no resources to clean up)."""
        pass
