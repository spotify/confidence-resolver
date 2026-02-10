"""Shared pytest fixtures for Confidence OpenFeature provider tests."""

from pathlib import Path
from typing import Optional, Protocol, Tuple

import pytest


# Path helpers
def get_repo_root() -> Path:
    """Get the repository root directory."""
    # tests/conftest.py -> python -> openfeature-provider -> repo root
    return Path(__file__).parent.parent.parent.parent


def get_data_dir() -> Path:
    """Get the data directory with test fixtures."""
    return get_repo_root() / "data"


# WASM fixtures
@pytest.fixture
def wasm_path() -> Path:
    """Path to the WASM binary."""
    return (
        Path(__file__).parent.parent / "resources" / "wasm" / "confidence_resolver.wasm"
    )


@pytest.fixture
def wasm_bytes(wasm_path: Path) -> bytes:
    """Load WASM binary bytes."""
    if not wasm_path.exists():
        pytest.skip(f"WASM binary not found at {wasm_path}")
    return wasm_path.read_bytes()


# Test state fixtures
@pytest.fixture
def test_resolver_state() -> bytes:
    """Load test resolver state from data directory."""
    state_path = get_data_dir() / "resolver_state_current.pb"
    if not state_path.exists():
        pytest.skip(f"Test resolver state not found at {state_path}")
    return state_path.read_bytes()


@pytest.fixture
def test_account_id() -> str:
    """Load test account ID."""
    account_path = get_data_dir() / "account_id"
    if not account_path.exists():
        pytest.skip(f"Test account ID not found at {account_path}")
    return account_path.read_text().strip()


@pytest.fixture
def test_client_secret() -> str:
    """Test client secret for the demo account."""
    return "mkjJruAATQWjeY7foFIWfVAcBWnci2YF"


# Mock protocols and classes
class FlagLoggerProtocol(Protocol):
    """Protocol for flag logging."""

    def write(self, request_bytes: bytes) -> None:
        """Write flag logs."""
        ...

    def shutdown(self) -> None:
        """Shutdown the logger."""
        ...


class StateFetcherProtocol(Protocol):
    """Protocol for state fetching."""

    def fetch(self) -> Tuple[bytes, str, bool]:
        """Fetch state, account ID, and whether state changed."""
        ...

    @property
    def state(self) -> Optional[bytes]:
        """Current cached state."""
        ...

    @property
    def account_id(self) -> Optional[str]:
        """Current cached account ID."""
        ...


class MockFlagLogger:
    """Mock flag logger for testing."""

    def __init__(self) -> None:
        self.writes: list[bytes] = []
        self.shutdown_called = False

    def write(self, request_bytes: bytes) -> None:
        self.writes.append(request_bytes)

    def shutdown(self) -> None:
        self.shutdown_called = True


class MockStateFetcher:
    """Mock state fetcher for testing."""

    def __init__(self, state: bytes, account_id: str) -> None:
        self._state = state
        self._account_id = account_id
        self.fetch_count = 0

    def fetch(self) -> Tuple[bytes, str, bool]:
        self.fetch_count += 1
        # Always return changed=True for first fetch, False for subsequent
        changed = self.fetch_count == 1
        return self._state, self._account_id, changed

    @property
    def state(self) -> Optional[bytes]:
        return self._state

    @property
    def account_id(self) -> Optional[str]:
        return self._account_id


@pytest.fixture
def mock_flag_logger() -> MockFlagLogger:
    """Create a mock flag logger."""
    return MockFlagLogger()


@pytest.fixture
def mock_state_fetcher(
    test_resolver_state: bytes, test_account_id: str
) -> MockStateFetcher:
    """Create a mock state fetcher with test data."""
    return MockStateFetcher(test_resolver_state, test_account_id)


# E2E test fixtures
@pytest.fixture
def e2e_client_secret() -> str:
    """Client secret for E2E tests."""
    return "Ip7lGcBeGA4Le9MI8md4i5LkUOnLnyFx"
