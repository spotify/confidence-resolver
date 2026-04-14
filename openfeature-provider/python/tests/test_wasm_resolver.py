"""Tests for WasmResolver class."""

import pytest
from google.protobuf import struct_pb2

from confidence.wasm_resolver import WasmResolver
from confidence.proto.confidence.wasm import wasm_api_pb2
from confidence.proto.confidence.flags.resolver.v1 import api_pb2

# Test constants - flag name from test fixture data
TEST_FLAG_NAME = "flags/tutorial-feature"


class TestWasmResolverInitialization:
    """Test WasmResolver initialization."""

    def test_init_with_valid_wasm_bytes(self, wasm_bytes: bytes) -> None:
        """WasmResolver initializes successfully with valid WASM binary."""
        resolver = WasmResolver(wasm_bytes)
        assert resolver is not None

    def test_init_with_invalid_wasm_bytes(self) -> None:
        """WasmResolver raises error with invalid WASM binary."""
        with pytest.raises(Exception):
            WasmResolver(b"invalid wasm bytes")


class TestSetResolverState:
    """Test set_resolver_state method."""

    def test_set_resolver_state_success(
        self, wasm_bytes: bytes, test_resolver_state: bytes, test_account_id: str
    ) -> None:
        """set_resolver_state succeeds with valid state and account_id."""
        resolver = WasmResolver(wasm_bytes)
        # Should not raise
        resolver.set_resolver_state(test_resolver_state, test_account_id)

    def test_set_resolver_state_empty_state(
        self, wasm_bytes: bytes, test_account_id: str
    ) -> None:
        """set_resolver_state handles empty state."""
        resolver = WasmResolver(wasm_bytes)
        # Empty state should be accepted (means no flags configured)
        resolver.set_resolver_state(b"", test_account_id)


class TestResolveProcess:
    """Test resolve_process method."""

    def test_resolve_process_returns_response(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """resolve_process returns a ResolveProcessResponse."""
        resolver = WasmResolver(wasm_bytes)
        resolver.set_resolver_state(test_resolver_state, test_account_id)

        # Create resolve request with valid flag and client secret
        resolve_request = api_pb2.ResolveFlagsRequest()
        resolve_request.flags.append(TEST_FLAG_NAME)
        resolve_request.client_secret = test_client_secret
        evaluation_context = struct_pb2.Struct()
        evaluation_context.fields["targeting_key"].string_value = "user-123"
        resolve_request.evaluation_context.CopyFrom(evaluation_context)

        request = wasm_api_pb2.ResolveProcessRequest()
        request.deferred_materializations.CopyFrom(resolve_request)

        response = resolver.resolve_process(request)

        assert response is not None
        assert isinstance(response, wasm_api_pb2.ResolveProcessResponse)

    def test_resolve_process_without_state_raises(
        self, wasm_bytes: bytes, test_client_secret: str
    ) -> None:
        """resolve_process raises error without state set."""
        resolver = WasmResolver(wasm_bytes)

        resolve_request = api_pb2.ResolveFlagsRequest()
        resolve_request.flags.append(TEST_FLAG_NAME)
        resolve_request.client_secret = test_client_secret

        request = wasm_api_pb2.ResolveProcessRequest()
        request.deferred_materializations.CopyFrom(resolve_request)

        # Without state, the WASM module will raise an error (client secret not found)
        with pytest.raises(RuntimeError):
            resolver.resolve_process(request)


class TestFlushLogs:
    """Test flush_logs method."""

    def test_flush_logs_returns_bytes(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """flush_logs returns bytes (WriteFlagLogsRequest serialized)."""
        resolver = WasmResolver(wasm_bytes)
        resolver.set_resolver_state(test_resolver_state, test_account_id)

        # Perform a resolve to generate logs
        resolve_request = api_pb2.ResolveFlagsRequest()
        resolve_request.flags.append(TEST_FLAG_NAME)
        resolve_request.client_secret = test_client_secret
        evaluation_context = struct_pb2.Struct()
        evaluation_context.fields["targeting_key"].string_value = "user-123"
        resolve_request.evaluation_context.CopyFrom(evaluation_context)

        request = wasm_api_pb2.ResolveProcessRequest()
        request.deferred_materializations.CopyFrom(resolve_request)
        resolver.resolve_process(request)

        logs = resolver.flush_logs()
        assert isinstance(logs, bytes)

    def test_flush_logs_empty_without_resolves(self, wasm_bytes: bytes) -> None:
        """flush_logs returns empty bytes when no resolves have been made."""
        resolver = WasmResolver(wasm_bytes)
        logs = resolver.flush_logs()
        assert isinstance(logs, bytes)


class TestFlushAssigned:
    """Test flush_assigned method."""

    def test_flush_assigned_returns_bytes(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """flush_assigned returns bytes."""
        resolver = WasmResolver(wasm_bytes)
        resolver.set_resolver_state(test_resolver_state, test_account_id)

        # Perform a resolve to generate assignments
        resolve_request = api_pb2.ResolveFlagsRequest()
        resolve_request.flags.append(TEST_FLAG_NAME)
        resolve_request.client_secret = test_client_secret
        evaluation_context = struct_pb2.Struct()
        evaluation_context.fields["targeting_key"].string_value = "user-123"
        resolve_request.evaluation_context.CopyFrom(evaluation_context)

        request = wasm_api_pb2.ResolveProcessRequest()
        request.deferred_materializations.CopyFrom(resolve_request)
        resolver.resolve_process(request)

        assigned = resolver.flush_assigned()
        assert isinstance(assigned, bytes)


class TestMemoryManagement:
    """Test memory allocation and deallocation."""

    @staticmethod
    def _build_request(client_secret: str) -> wasm_api_pb2.ResolveProcessRequest:
        resolve_request = api_pb2.ResolveFlagsRequest()
        resolve_request.flags.append(TEST_FLAG_NAME)
        resolve_request.client_secret = client_secret
        evaluation_context = struct_pb2.Struct()
        evaluation_context.fields["targeting_key"].string_value = "user-123"
        resolve_request.evaluation_context.CopyFrom(evaluation_context)
        request = wasm_api_pb2.ResolveProcessRequest()
        request.deferred_materializations.CopyFrom(resolve_request)
        return request

    def test_multiple_resolves_dont_leak_memory(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """WASM memory must not grow on repeated resolves.

        Each resolve_flags call triggers a current_time() host call which
        allocates a request in WASM memory. If the host doesn't free it,
        memory leaks ~20 bytes per call and eventually forces memory.grow.
        """
        resolver = WasmResolver(wasm_bytes)
        resolver.set_resolver_state(test_resolver_state, test_account_id)
        request = self._build_request(test_client_secret)

        # Warm up to settle one-time allocations
        for i in range(50_000):
            resolver.resolve_process(request)
            if i % 1000 == 0:
                resolver.flush_logs()

        pages_before = resolver._memory.size(resolver._store)

        for i in range(50_000):
            resolver.resolve_process(request)
            if i % 1000 == 0:
                resolver.flush_logs()

        pages_after = resolver._memory.size(resolver._store)

        assert pages_after == pages_before, (
            f"WASM memory grew from {pages_before} to {pages_after} pages "
            f"({(pages_after - pages_before) * 65536} bytes leaked) — "
            f"host function is not freeing guest request allocations"
        )
