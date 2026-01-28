"""Tests for LocalResolver class (wrapper with crash recovery)."""

import pytest
from unittest.mock import Mock
from google.protobuf import struct_pb2

from confidence.local_resolver import LocalResolver
from confidence.wasm_resolver import WasmResolver
from confidence.proto.confidence.wasm import wasm_api_pb2
from confidence.proto.confidence.flags.resolver.v1 import api_pb2

# Test constants - flag name from test fixture data
TEST_FLAG_NAME = "flags/tutorial-feature"


class TestLocalResolverInitialization:
    """Test LocalResolver initialization."""

    def test_init_with_wasm_bytes(self, wasm_bytes: bytes) -> None:
        """LocalResolver initializes with WASM bytes."""
        resolver = LocalResolver(wasm_bytes)
        assert resolver is not None

    def test_init_creates_wasm_resolver(self, wasm_bytes: bytes) -> None:
        """LocalResolver creates a WasmResolver internally."""
        resolver = LocalResolver(wasm_bytes)
        assert resolver._delegate is not None


class TestLocalResolverSetState:
    """Test LocalResolver state management."""

    def test_set_resolver_state_stores_state(
        self, wasm_bytes: bytes, test_resolver_state: bytes, test_account_id: str
    ) -> None:
        """set_resolver_state stores state for recovery."""
        resolver = LocalResolver(wasm_bytes)
        resolver.set_resolver_state(test_resolver_state, test_account_id)

        assert resolver._current_state is not None
        assert resolver._current_state == (test_resolver_state, test_account_id)

    def test_set_resolver_state_delegates_to_wasm(
        self, wasm_bytes: bytes, test_resolver_state: bytes, test_account_id: str
    ) -> None:
        """set_resolver_state calls underlying WasmResolver."""
        resolver = LocalResolver(wasm_bytes)
        resolver.set_resolver_state(test_resolver_state, test_account_id)
        # If it doesn't raise, the call succeeded


class TestLocalResolverResolve:
    """Test LocalResolver resolve functionality."""

    def test_resolve_with_sticky_delegates(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """resolve_with_sticky delegates to WasmResolver."""
        resolver = LocalResolver(wasm_bytes)
        resolver.set_resolver_state(test_resolver_state, test_account_id)

        resolve_request = api_pb2.ResolveFlagsRequest()
        resolve_request.flags.append(TEST_FLAG_NAME)
        resolve_request.client_secret = test_client_secret
        evaluation_context = struct_pb2.Struct()
        evaluation_context.fields["targeting_key"].string_value = "user-123"
        resolve_request.evaluation_context.CopyFrom(evaluation_context)

        request = wasm_api_pb2.ResolveWithStickyRequest()
        request.resolve_request.CopyFrom(resolve_request)

        response = resolver.resolve_with_sticky(request)
        assert response is not None


class TestLocalResolverCrashRecovery:
    """Test LocalResolver crash recovery functionality."""

    def test_reload_on_runtime_error_during_resolve(
        self, wasm_bytes: bytes, test_resolver_state: bytes, test_account_id: str
    ) -> None:
        """LocalResolver reloads WASM on RuntimeError during resolve."""
        resolver = LocalResolver(wasm_bytes)
        resolver.set_resolver_state(test_resolver_state, test_account_id)

        # Mock the delegate to raise RuntimeError
        mock_delegate = Mock(spec=WasmResolver)
        mock_delegate.resolve_with_sticky.side_effect = RuntimeError(
            "WASM trap: unreachable"
        )
        mock_delegate.flush_logs.return_value = b""
        resolver._delegate = mock_delegate

        request = wasm_api_pb2.ResolveWithStickyRequest()

        # Should raise the error but also reload
        with pytest.raises(RuntimeError):
            resolver.resolve_with_sticky(request)

        # Delegate should be replaced
        assert resolver._delegate is not mock_delegate

    def test_state_restored_after_recovery(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """After recovery, state is restored to new WASM instance."""
        resolver = LocalResolver(wasm_bytes)
        resolver.set_resolver_state(test_resolver_state, test_account_id)

        # Force a reload by simulating error
        old_delegate = resolver._delegate
        resolver._reload_instance(RuntimeError("simulated"))

        # New delegate should be different
        assert resolver._delegate is not old_delegate

        # State should be reapplied (resolve should still work)
        resolve_request = api_pb2.ResolveFlagsRequest()
        resolve_request.flags.append(TEST_FLAG_NAME)
        resolve_request.client_secret = test_client_secret
        evaluation_context = struct_pb2.Struct()
        evaluation_context.fields["targeting_key"].string_value = "user-123"
        resolve_request.evaluation_context.CopyFrom(evaluation_context)

        request = wasm_api_pb2.ResolveWithStickyRequest()
        request.resolve_request.CopyFrom(resolve_request)

        # Should work after recovery
        response = resolver.resolve_with_sticky(request)
        assert response is not None


class TestLocalResolverFlushLogs:
    """Test LocalResolver flush_logs with buffering."""

    def test_flush_logs_includes_buffered(
        self, wasm_bytes: bytes, test_resolver_state: bytes, test_account_id: str
    ) -> None:
        """flush_logs includes buffered logs from before crash."""
        resolver = LocalResolver(wasm_bytes)
        resolver.set_resolver_state(test_resolver_state, test_account_id)

        # Add some buffered logs manually
        resolver._buffered_logs.append(b"buffered-log-data")

        logs = resolver.flush_logs()
        assert b"buffered-log-data" in logs

    def test_flush_logs_clears_buffer(
        self, wasm_bytes: bytes, test_resolver_state: bytes, test_account_id: str
    ) -> None:
        """flush_logs clears the buffer after returning."""
        resolver = LocalResolver(wasm_bytes)
        resolver.set_resolver_state(test_resolver_state, test_account_id)

        resolver._buffered_logs.append(b"buffered-log-data")
        resolver.flush_logs()

        assert len(resolver._buffered_logs) == 0

    def test_flush_logs_attempts_to_buffer_on_crash(
        self, wasm_bytes: bytes, test_resolver_state: bytes, test_account_id: str
    ) -> None:
        """When crashing, LocalResolver tries to buffer logs before reload."""
        resolver = LocalResolver(wasm_bytes)
        resolver.set_resolver_state(test_resolver_state, test_account_id)

        # Create a mock that returns logs first, then raises
        mock_delegate = Mock(spec=WasmResolver)
        mock_delegate.flush_logs.return_value = b"crash-logs"
        resolver._delegate = mock_delegate

        # Simulate reload - should try to flush logs first
        resolver._reload_instance(RuntimeError("test"))

        # buffered_logs should contain the flushed logs
        assert b"crash-logs" in resolver._buffered_logs


class TestLocalResolverFlushAssigned:
    """Test LocalResolver flush_assigned."""

    def test_flush_assigned_delegates(
        self, wasm_bytes: bytes, test_resolver_state: bytes, test_account_id: str
    ) -> None:
        """flush_assigned delegates to WasmResolver."""
        resolver = LocalResolver(wasm_bytes)
        resolver.set_resolver_state(test_resolver_state, test_account_id)

        result = resolver.flush_assigned()
        assert isinstance(result, bytes)
