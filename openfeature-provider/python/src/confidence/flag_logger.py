"""Flag logger implementations for Confidence OpenFeature provider.

This module provides flag logging functionality to send flag assignment events
to the Confidence backend via gRPC or HTTP.
"""

import json
import logging
import threading
from concurrent.futures import ThreadPoolExecutor
from typing import List, Optional, Protocol, runtime_checkable

import grpc
import httpx

from confidence.proto.confidence.flags.resolver.v1 import (
    internal_api_pb2,
    internal_api_pb2_grpc,
)
from confidence.proto.confidence.flags.admin.v1.resolver_pb2 import (
    LOG_DESTINATION_CLOUDFLARE,
    LOG_DESTINATION_SPOTIFY_EDGE,
)

logger = logging.getLogger(__name__)

# gRPC target for the Confidence edge service
GRPC_TARGET = "edge-grpc.spotify.com:443"

# HTTP endpoint for the Cloudflare flag log ingestor
CLOUDFLARE_INGEST_URL = (
    "https://epx-flags-logs.experimentation-platform.workers.dev/v1/flagLogs:ingest"
)

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
                    "initialBackoff": "1s",
                    "maxBackoff": "10s",
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
                    logger.warning("Flag log write failures: %d/10", self._failures)
                self._failures = 0

    def shutdown(self) -> None:
        """Shutdown the logger and wait for pending writes to complete."""
        self._executor.shutdown(wait=True)
        if self._owns_channel:
            self._channel.close()


class HttpFlagLogger:
    """HTTP-based flag logger that sends flag logs to the Cloudflare ingestor.

    Wraps WriteFlagLogsRequest in an IngestFlagLogsRequest with the account ID,
    serializes as protobuf, and POSTs to the Cloudflare endpoint.

    Writes are performed asynchronously using a thread pool.
    """

    def __init__(
        self,
        client_secret: str,
        account_id: str,
        http_client: Optional[httpx.Client] = None,
    ) -> None:
        """Initialize the HTTP flag logger.

        Args:
            client_secret: The Confidence client secret for authentication.
            account_id: The account ID to include in IngestFlagLogsRequest.
            http_client: Optional httpx.Client for custom HTTP configuration or testing.
        """
        self._client_secret = client_secret
        self._account_id = account_id
        self._executor = ThreadPoolExecutor(max_workers=2)
        self._stats_lock = threading.Lock()
        self._attempts = 0
        self._failures = 0

        if http_client is not None:
            self._http_client = http_client
            self._owns_client = False
        else:
            self._http_client = httpx.Client(timeout=30.0)
            self._owns_client = True

    def set_account_id(self, account_id: str) -> None:
        """Update the account ID used in IngestFlagLogsRequest.

        Args:
            account_id: The new account ID.
        """
        self._account_id = account_id

    def write(self, request_bytes: bytes) -> None:
        """Write flag logs asynchronously via HTTP.

        Skips empty requests (no data).

        Args:
            request_bytes: Serialized WriteFlagLogsRequest proto bytes.
        """
        if not request_bytes:
            logger.debug("Skipping empty flag log request (empty bytes)")
            return

        try:
            request = internal_api_pb2.WriteFlagLogsRequest()
            request.ParseFromString(request_bytes)
        except Exception as e:
            logger.error("Failed to parse WriteFlagLogsRequest: %s", e)
            return

        if (
            len(request.flag_assigned) == 0
            and len(request.client_resolve_info) == 0
            and len(request.flag_resolve_info) == 0
        ):
            logger.debug("Skipping empty flag log request (no data)")
            return

        self._executor.submit(self._send_request, request)

    def _send_request(self, request: internal_api_pb2.WriteFlagLogsRequest) -> None:
        """Send the request via HTTP POST (runs in thread pool).

        Args:
            request: The WriteFlagLogsRequest to send.
        """
        failed = False
        try:
            ingest_request = internal_api_pb2.IngestFlagLogsRequest()
            ingest_request.account_id = self._account_id
            ingest_request.batch.CopyFrom(request)

            body = ingest_request.SerializeToString()
            headers = {
                "Authorization": f"ClientSecret {self._client_secret}",
                "Content-Type": "application/protobuf",
            }
            response = self._http_client.post(
                CLOUDFLARE_INGEST_URL,
                content=body,
                headers=headers,
            )
            if response.status_code >= 400:
                logger.warning(
                    "Cloudflare flag log ingest returned HTTP %d",
                    response.status_code,
                )
                failed = True
            else:
                logger.debug(
                    "Successfully sent flag log via HTTP with %d entries",
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
                        "HTTP flag log write failures: %d/10", self._failures
                    )
                self._failures = 0

    def shutdown(self) -> None:
        """Shutdown the logger and wait for pending writes to complete."""
        self._executor.shutdown(wait=True)
        if self._owns_client:
            self._http_client.close()


class MultiDestinationFlagLogger:
    """Flag logger that routes logs to multiple destinations with fallback.

    The first destination is primary. If it fails, the second destination
    is used as fallback. Destinations are determined by the log_destinations
    from the CDN state.
    """

    def __init__(
        self,
        client_secret: str,
        account_id: str,
        log_destinations: List[int],
        grpc_channel: Optional[grpc.Channel] = None,
        http_client: Optional[httpx.Client] = None,
    ) -> None:
        """Initialize the multi-destination flag logger.

        Args:
            client_secret: The Confidence client secret for authentication.
            account_id: The account ID (needed for Cloudflare destination).
            log_destinations: Ordered list of LogDestination enum values.
                First is primary, second is fallback.
            grpc_channel: Optional gRPC channel for testing.
            http_client: Optional httpx.Client for testing.
        """
        self._client_secret = client_secret
        self._account_id = account_id
        self._grpc_channel = grpc_channel
        self._http_client = http_client

        self._loggers = self._build_loggers(log_destinations)

    def _build_loggers(self, destinations: List[int]) -> List[FlagLogger]:
        """Build logger instances for each destination.

        Args:
            destinations: List of LogDestination enum values.

        Returns:
            List of FlagLogger instances.
        """
        loggers: List[FlagLogger] = []
        for dest in destinations:
            if dest == LOG_DESTINATION_CLOUDFLARE:
                loggers.append(
                    HttpFlagLogger(
                        client_secret=self._client_secret,
                        account_id=self._account_id,
                        http_client=self._http_client,
                    )
                )
            elif dest == LOG_DESTINATION_SPOTIFY_EDGE:
                loggers.append(
                    GrpcFlagLogger(
                        client_secret=self._client_secret,
                        channel=self._grpc_channel,
                    )
                )
            else:
                logger.debug("Ignoring unknown log destination: %d", dest)
        return loggers

    def write(self, request_bytes: bytes) -> None:
        """Write flag logs to the primary destination, falling back on error.

        Args:
            request_bytes: Serialized WriteFlagLogsRequest proto bytes.
        """
        if not self._loggers:
            return

        # Primary destination
        try:
            self._loggers[0].write(request_bytes)
            return
        except Exception as e:
            logger.warning("Primary flag log destination failed: %s", e)

        # Fallback destinations
        for fallback in self._loggers[1:]:
            try:
                fallback.write(request_bytes)
                return
            except Exception as e:
                logger.warning("Fallback flag log destination failed: %s", e)

    def set_account_id(self, account_id: str) -> None:
        """Update the account ID on any HttpFlagLogger children.

        Args:
            account_id: The new account ID.
        """
        self._account_id = account_id
        for lg in self._loggers:
            if isinstance(lg, HttpFlagLogger):
                lg.set_account_id(account_id)

    def shutdown(self) -> None:
        """Shutdown all loggers and wait for pending writes to complete."""
        for lg in self._loggers:
            lg.shutdown()


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
