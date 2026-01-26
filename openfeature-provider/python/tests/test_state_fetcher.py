"""Tests for the CDN state fetcher."""

import hashlib

import httpx
import pytest
from pytest_httpx import HTTPXMock

from confidence_openfeature.proto.confidence.wasm.messages_pb2 import (
    SetResolverStateRequest,
)
from confidence_openfeature.state_fetcher import StateFetcher

CDN_BASE_URL = "https://confidence-resolver-state-cdn.spotifycdn.com"


def get_cdn_url(client_secret: str) -> str:
    """Build the CDN URL from the client secret."""
    hash_hex = hashlib.sha256(client_secret.encode()).hexdigest()
    return f"{CDN_BASE_URL}/{hash_hex}"


class TestNewFetcher:
    """Test StateFetcher creation."""

    def test_new_fetcher(self) -> None:
        """Test that fetcher is created correctly."""
        fetcher = StateFetcher("test-client-secret")

        assert fetcher is not None
        assert fetcher._client_secret == "test-client-secret"

    def test_new_fetcher_with_custom_client(self) -> None:
        """Test fetcher with custom HTTP client."""
        custom_client = httpx.Client(timeout=60.0)
        fetcher = StateFetcher("test-client-secret", http_client=custom_client)

        assert fetcher._http_client is custom_client
        custom_client.close()


class TestGetRawStateInitiallyEmpty:
    """Test that state and account_id are None before fetch."""

    def test_state_initially_none(self) -> None:
        """State should be None before first fetch."""
        fetcher = StateFetcher("test-client-secret")
        assert fetcher.state is None

    def test_account_id_initially_none(self) -> None:
        """Account ID should be None before first fetch."""
        fetcher = StateFetcher("test-client-secret")
        assert fetcher.account_id is None


class TestReloadSuccess:
    """Test successful state fetching."""

    def test_reload_success(self, httpx_mock: HTTPXMock) -> None:
        """Test fetching and parsing state correctly."""
        client_secret = "test-client-secret"
        test_state = b"test-resolver-state-bytes"
        test_account_id = "test-account-123"

        # Create protobuf response
        state_request = SetResolverStateRequest()
        state_request.state = test_state
        state_request.account_id = test_account_id
        response_bytes = state_request.SerializeToString()

        # Mock the HTTP response
        httpx_mock.add_response(
            url=get_cdn_url(client_secret),
            content=response_bytes,
            headers={"ETag": "test-etag"},
        )

        fetcher = StateFetcher(client_secret)
        state, account_id = fetcher.fetch()

        assert state == test_state
        assert account_id == test_account_id
        assert fetcher.state == test_state
        assert fetcher.account_id == test_account_id


class TestReloadNotModified:
    """Test 304 Not Modified returns cached state."""

    def test_reload_not_modified(self, httpx_mock: HTTPXMock) -> None:
        """Test that 304 returns cached state."""
        client_secret = "test-client-secret"
        test_state = b"test-resolver-state-bytes"
        test_account_id = "test-account-123"

        # Create protobuf response
        state_request = SetResolverStateRequest()
        state_request.state = test_state
        state_request.account_id = test_account_id
        response_bytes = state_request.SerializeToString()

        cdn_url = get_cdn_url(client_secret)

        # First request returns state with ETag
        httpx_mock.add_response(
            url=cdn_url,
            content=response_bytes,
            headers={"ETag": "test-etag"},
        )

        fetcher = StateFetcher(client_secret)
        state1, account_id1 = fetcher.fetch()

        # Second request returns 304 Not Modified
        httpx_mock.add_response(
            url=cdn_url,
            status_code=304,
        )

        state2, account_id2 = fetcher.fetch()

        # Should return cached values
        assert state2 == state1
        assert account_id2 == account_id1


class TestReloadServerError:
    """Test 5xx server errors raise exceptions."""

    def test_reload_server_error(self, httpx_mock: HTTPXMock) -> None:
        """Test that 5xx raises exception."""
        client_secret = "test-client-secret"

        httpx_mock.add_response(
            url=get_cdn_url(client_secret),
            status_code=500,
        )

        fetcher = StateFetcher(client_secret)

        with pytest.raises(Exception) as exc_info:
            fetcher.fetch()

        assert "500" in str(exc_info.value)


class TestReloadClientError:
    """Test 4xx client errors raise exceptions."""

    def test_reload_client_error(self, httpx_mock: HTTPXMock) -> None:
        """Test that 4xx raises exception."""
        client_secret = "test-client-secret"

        httpx_mock.add_response(
            url=get_cdn_url(client_secret),
            status_code=404,
        )

        fetcher = StateFetcher(client_secret)

        with pytest.raises(Exception) as exc_info:
            fetcher.fetch()

        assert "404" in str(exc_info.value)


class TestEtagSentOnSecondRequest:
    """Test If-None-Match header is sent on second request."""

    def test_etag_sent_on_second_request(self, httpx_mock: HTTPXMock) -> None:
        """Test that If-None-Match header is sent after first fetch."""
        client_secret = "test-client-secret"
        test_state = b"test-resolver-state-bytes"
        test_account_id = "test-account-123"

        # Create protobuf response
        state_request = SetResolverStateRequest()
        state_request.state = test_state
        state_request.account_id = test_account_id
        response_bytes = state_request.SerializeToString()

        cdn_url = get_cdn_url(client_secret)

        # First request returns state with ETag
        httpx_mock.add_response(
            url=cdn_url,
            content=response_bytes,
            headers={"ETag": '"test-etag-value"'},
        )

        fetcher = StateFetcher(client_secret)
        fetcher.fetch()

        # Second request should include If-None-Match
        httpx_mock.add_response(
            url=cdn_url,
            status_code=304,
        )

        fetcher.fetch()

        # Verify the If-None-Match header was sent on second request
        requests = httpx_mock.get_requests()
        assert len(requests) == 2

        # First request should not have If-None-Match
        assert "If-None-Match" not in requests[0].headers

        # Second request should have If-None-Match with the ETag value
        assert requests[1].headers.get("If-None-Match") == '"test-etag-value"'
