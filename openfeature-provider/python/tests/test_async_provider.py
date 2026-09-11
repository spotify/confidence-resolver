"""Async evaluations must leave the event loop responsive during contention."""

import asyncio
import threading
from typing import Any
from unittest.mock import Mock

import pytest
from openfeature.evaluation_context import EvaluationContext
from openfeature.exception import ErrorCode
from openfeature.flag_evaluation import Reason

from confidence.local_resolver import LocalResolver
from confidence.provider import ConfidenceProvider
from confidence.proto.confidence.flags.resolver.v1 import types_pb2
from confidence.proto.confidence.wasm import wasm_api_pb2


@pytest.mark.parametrize(
    "kind,default,value",
    [
        ("boolean", False, True),
        ("string", "fallback", "resolved"),
        ("integer", 0, 42),
        ("float", 0.0, 1.5),
        ("object", {}, {"enabled": True}),
    ],
)
@pytest.mark.parametrize("missing", [False, True], ids=["matched", "missing"])
async def test_async_resolution_during_contention(
    kind: str, default: Any, value: Any, missing: bool
) -> None:
    provider = ConfidenceProvider(client_secret="test-secret", encryption_key="00" * 32)
    resolver = Mock(spec=LocalResolver)
    provider._resolver = resolver
    resolver.flush_logs.return_value = b""
    response = wasm_api_pb2.ResolveProcessResponse()
    response.resolved.SetInParent()
    if not missing:
        flag = response.resolved.response.resolved_flags.add(
            flag="flags/test",
            variant="flags/test/variants/on",
            reason=types_pb2.RESOLVE_REASON_MATCH,
        )
        flag.value.update({"value": value})

    loop_thread = threading.get_ident()
    loop_progressed = threading.Event()

    def resolve(request: wasm_api_pb2.ResolveProcessRequest) -> Any:
        assert threading.get_ident() != loop_thread
        assert loop_progressed.is_set()
        assert provider._resolver_lock.locked()
        assert (
            request.without_materializations.evaluation_context["targeting_key"]
            == "user"
        )
        return response

    resolver.resolve_process.side_effect = resolve
    provider._resolver_lock.acquire()
    # A separate thread releases the lock even if a regression blocks the loop.
    release = threading.Timer(0.05, provider._resolver_lock.release)
    release.start()
    asyncio.get_running_loop().call_soon(loop_progressed.set)
    try:
        result = await getattr(provider, f"resolve_{kind}_details_async")(
            "test.value", default, EvaluationContext(targeting_key="user")
        )
        assert result.value == (default if missing else value)
        assert result.error_code == (ErrorCode.FLAG_NOT_FOUND if missing else None)
        assert result.reason == (Reason.ERROR if missing else Reason.TARGETING_MATCH)
        assert result.variant == (None if missing else "flags/test/variants/on")
        resolver.resolve_process.assert_called_once()
        resolver.register_resolve.assert_called_once()
    finally:
        release.join()
        provider.shutdown()
