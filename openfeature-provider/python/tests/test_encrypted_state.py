"""Tests for encrypted resolver state."""

import pytest
from google.protobuf import struct_pb2

from confidence.proto.confidence.flags.resolver.v1 import api_pb2, types_pb2
from confidence.proto.confidence.wasm import wasm_api_pb2
from confidence.wasm_resolver import WasmResolver
from confidence.local_resolver import LocalResolver


CLIENT_SECRET = "mkjJruAATQWjeY7foFIWfVAcBWnci2YF"


class TestWasmResolverEncryptedState:
    def test_resolves_flags_after_decrypting_state(
        self,
        wasm_bytes: bytes,
        test_encrypted_state: bytes,
        test_encryption_key: bytes,
    ) -> None:
        resolver = WasmResolver(wasm_bytes)
        sdk = types_pb2.Sdk(
            id=types_pb2.SdkId.SDK_ID_PYTHON_PROVIDER, version="0.0.0-test"
        )
        resolver.set_encrypted_resolver_state(
            test_encrypted_state, test_encryption_key, sdk
        )

        ctx = struct_pb2.Struct()
        ctx.fields["targeting_key"].string_value = "tutorial_visitor"
        ctx.fields["visitor_id"].string_value = "tutorial_visitor"

        resolve_req = api_pb2.ResolveFlagsRequest()
        resolve_req.flags.append("flags/tutorial-feature")
        resolve_req.client_secret = CLIENT_SECRET
        resolve_req.evaluation_context.CopyFrom(ctx)
        resolve_req.apply = True

        request = wasm_api_pb2.ResolveProcessRequest()
        request.deferred_materializations.CopyFrom(resolve_req)

        response = resolver.resolve_process(request)
        assert response.HasField("resolved")
        assert len(response.resolved.response.resolved_flags) == 1
        assert (
            response.resolved.response.resolved_flags[0].reason
            == types_pb2.RESOLVE_REASON_MATCH
        )

    def test_rejects_wrong_encryption_key(
        self,
        wasm_bytes: bytes,
        test_encrypted_state: bytes,
    ) -> None:
        resolver = WasmResolver(wasm_bytes)
        wrong_key = bytes(32)
        with pytest.raises(RuntimeError, match="decrypt"):
            resolver.set_encrypted_resolver_state(test_encrypted_state, wrong_key)


class TestLocalResolverEncryptedState:
    def test_caches_encrypted_state_for_recovery(
        self,
        wasm_bytes: bytes,
        test_encrypted_state: bytes,
        test_encryption_key: bytes,
    ) -> None:
        resolver = LocalResolver(wasm_bytes)
        resolver.set_encrypted_resolver_state(test_encrypted_state, test_encryption_key)
        assert resolver._current_encrypted_state is not None
        assert resolver._current_state is None
