"""CDN state fetcher for Confidence OpenFeature provider.

This module provides functionality to fetch resolver state from the Confidence CDN.
"""

import hashlib
import logging
from typing import Optional, Tuple

import httpx

from confidence.proto.confidence.wasm.messages_pb2 import (
    SetResolverStateRequest,
)

logger = logging.getLogger(__name__)

CDN_BASE_URL = "https://confidence-resolver-state-cdn.spotifycdn.com"


class StateFetcherError(Exception):
    """Exception raised when state fetching fails."""

    pass


class StateFetcher:
    """Fetches and caches resolver state from the Confidence CDN.

    The StateFetcher uses ETag-based caching to efficiently poll for state updates.
    It builds the CDN URL from a SHA256 hash of the client secret.

    Example:
        fetcher = StateFetcher("your-client-secret")
        state, account_id = fetcher.fetch()
    """

    def __init__(
        self,
        client_secret: str,
        http_client: Optional[httpx.Client] = None,
    ) -> None:
        """Initialize the StateFetcher.

        Args:
            client_secret: The Confidence client secret used to build the CDN URL.
            http_client: Optional httpx.Client for custom HTTP configuration or testing.
        """
        self._client_secret = client_secret
        self._http_client = http_client
        self._owns_client = http_client is None

        # Cached state
        self._state: Optional[bytes] = None
        self._account_id: Optional[str] = None
        self._etag: Optional[str] = None

        # Build CDN URL from SHA256 hash of client secret
        hash_hex = hashlib.sha256(client_secret.encode()).hexdigest()
        self._cdn_url = f"{CDN_BASE_URL}/{hash_hex}"

    @property
    def state(self) -> Optional[bytes]:
        """Return the current cached resolver state."""
        return self._state

    @property
    def account_id(self) -> Optional[str]:
        """Return the current cached account ID."""
        return self._account_id

    def _get_client(self) -> httpx.Client:
        """Get or create the HTTP client."""
        if self._http_client is not None:
            return self._http_client
        # Create a new client for this request
        return httpx.Client(timeout=30.0)

    def fetch(self) -> Tuple[bytes, str, bool]:
        """Fetch the resolver state from the CDN.

        This method fetches the latest resolver state from the Confidence CDN.
        It uses ETag-based caching to avoid re-downloading unchanged state.

        Returns:
            A tuple of (state_bytes, account_id, changed) where changed indicates
            whether the state was updated (False means 304 Not Modified).

        Raises:
            StateFetcherError: If the HTTP request fails or returns an error status.
        """
        client = self._get_client()
        should_close = self._http_client is None

        try:
            headers = {}
            if self._etag is not None:
                headers["If-None-Match"] = self._etag

            response = client.get(self._cdn_url, headers=headers)

            # Handle 304 Not Modified - return cached state
            if response.status_code == 304:
                if self._state is not None and self._account_id is not None:
                    logger.debug("State not modified (304), using cached state")
                    return self._state, self._account_id, False
                raise StateFetcherError(
                    "Received 304 Not Modified but no cached state available"
                )

            # Handle error responses
            if response.status_code != 200:
                raise StateFetcherError(
                    f"Failed to fetch state: HTTP {response.status_code}"
                )

            # Parse the protobuf response
            state_request = SetResolverStateRequest()
            state_request.ParseFromString(response.content)

            # Cache the state, account ID, and ETag
            self._state = state_request.state
            self._account_id = state_request.account_id
            self._etag = response.headers.get("ETag")

            logger.info(
                "Loaded resolver state for account=%s, etag=%s",
                self._account_id,
                self._etag,
            )

            return self._state, self._account_id, True

        finally:
            if should_close:
                client.close()
