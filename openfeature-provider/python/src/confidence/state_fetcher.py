"""CDN state fetcher for Confidence OpenFeature provider.

This module provides functionality to fetch resolver state from the Confidence CDN.
"""

import hashlib
import logging
import re
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from typing import List, Optional, Tuple

import httpx

from confidence.proto.confidence.flags.admin.v1.resolver_pb2 import (
    ClientResolverState,
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

    A fetched state is cached (and its ETag sent on the next request) only
    after the consumer installs it via mark_applied(), so a state whose
    install failed is re-fetched instead of being 304-skipped (#675).

    Example:
        fetcher = StateFetcher("your-client-secret", "your-encryption-key")
        state, account_id = fetcher.fetch()
    """

    def __init__(
        self,
        client_secret: str,
        encryption_key: str,
        http_client: Optional[httpx.Client] = None,
    ) -> None:
        """Initialize the StateFetcher.

        Args:
            client_secret: The Confidence client secret used to build the CDN URL.
            http_client: Optional httpx.Client for custom HTTP configuration or testing.
        """
        validate_encryption_key(encryption_key)
        self._client_secret = client_secret
        self._encryption_key = encryption_key
        self._http_client = http_client
        self._owns_client = http_client is None

        # Cached state: the latest state the consumer installed via
        # mark_applied(). A 304 is answered from this cache and its ETag is
        # the only one sent as If-None-Match.
        self._state: Optional[bytes] = None
        self._account_id: Optional[str] = None
        self._log_destinations: List[int] = []
        self._etag: Optional[str] = None
        # Most recent fetch, staged until the consumer installs it (#675).
        self._pending: Optional[Tuple[bytes, str, List[int], Optional[str]]] = None

        # Build CDN URL from SHA256 hash of client secret
        hash_hex = hashlib.sha256(client_secret.encode()).hexdigest()
        self._cdn_url = f"{CDN_BASE_URL}/{hash_hex}.enc"

    @property
    def state(self) -> Optional[bytes]:
        """Return the resolver state last committed via mark_applied()."""
        return self._state

    @property
    def account_id(self) -> Optional[str]:
        """Return the current cached account ID."""
        return self._account_id

    @property
    def log_destinations(self) -> List[int]:
        """Return the current cached log destinations."""
        return self._log_destinations

    def _get_client(self) -> httpx.Client:
        """Get or create the HTTP client."""
        if self._http_client is not None:
            return self._http_client
        # Create a new client for this request
        return httpx.Client(timeout=30.0)

    def fetch(self) -> Tuple[bytes, str, bool, List[int]]:
        """Fetch the resolver state from the CDN.

        This method fetches the latest resolver state from the Confidence CDN.
        It uses ETag-based caching to avoid re-downloading unchanged state.

        Returns:
            A tuple of (state_bytes, account_id, changed, log_destinations) where
            changed indicates whether the state was updated (False means 304 Not
            Modified) and log_destinations is a list of LogDestination enum values.
            When changed is True the caller must install the state and then call
            mark_applied(); until then nothing is cached and no ETag is sent.

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
                    return (
                        self._state,
                        self._account_id,
                        False,
                        self._log_destinations,
                    )
                raise StateFetcherError(
                    "Received 304 Not Modified but no cached state available"
                )

            # Handle error responses
            if response.status_code != 200:
                raise StateFetcherError(
                    f"Failed to fetch state: HTTP {response.status_code}"
                )

            content = _decrypt_aes_gcm(response.content, self._encryption_key)

            client_state = ClientResolverState()
            client_state.ParseFromString(content)
            state = client_state.state
            account_id = client_state.account
            log_destinations = list(client_state.log_destinations)
            etag = response.headers.get("ETag")
            # Stage the fetched state; the cache and the ETag sent on the
            # next request advance only when the caller installs it via
            # mark_applied(). A failed install must be re-fetched, not
            # 304-skipped (#675).
            self._pending = (state, account_id, log_destinations, etag)
            logger.info(
                "Loaded resolver state for account=%s, etag=%s, log_destinations=%s",
                account_id,
                etag,
                log_destinations,
            )
            return state, account_id, True, log_destinations

        finally:
            if should_close:
                client.close()

    def mark_applied(self) -> None:
        """Commit the fetched state as installed.

        Call after set_resolver_state succeeds. Only then do the cached state
        and ETag advance, mirroring the JS provider (stateEtag is stored only
        after setResolverState succeeds) and Go (appliedState advances only
        on success). If the install failed, nothing is committed and the next
        fetch re-downloads the state instead of being 304-skipped (#675).
        """
        if self._pending is None:
            return
        self._state, self._account_id, self._log_destinations, self._etag = (
            self._pending
        )
        self._pending = None


def _decrypt_aes_gcm(data: bytes, hex_key: str) -> bytes:
    key = bytes.fromhex(hex_key)
    nonce_len = 12
    if len(data) < nonce_len:
        raise StateFetcherError("Encrypted state too short (missing nonce)")
    nonce = data[:nonce_len]
    ciphertext = data[nonce_len:]
    return AESGCM(key).decrypt(nonce, ciphertext, None)


def validate_encryption_key(key: str) -> None:
    """Reject invalid AES-256 configuration without exposing the supplied key."""
    if not isinstance(key, str) or re.fullmatch(r"[0-9a-fA-F]{64}", key) is None:
        raise ValueError(
            "encryption_key is required and must contain exactly 64 hexadecimal characters"
        )
