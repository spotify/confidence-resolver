"""Tests for ConfidenceProvider class."""

from openfeature.evaluation_context import EvaluationContext
from openfeature.exception import ErrorCode
from openfeature.flag_evaluation import FlagResolutionDetails, Reason

from confidence.provider import ConfidenceProvider
from confidence.proto.confidence.flags.resolver.v1 import internal_api_pb2, types_pb2
from confidence.version import __version__
from tests.conftest import MockFlagLogger, MockStateFetcher


class TestGetMetadata:
    """Tests for provider metadata."""

    def test_get_metadata(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """Test that get_metadata returns correct provider name."""
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
        )

        metadata = provider.get_metadata()
        assert metadata.name == "confidence-sdk-python-local"


class TestInitialize:
    """Tests for provider initialization."""

    def test_init_telemetry_includes_sdk(
        self,
        wasm_bytes: bytes,
        test_client_secret: str,
    ) -> None:
        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            flag_logger=MockFlagLogger(),
            wasm_bytes=wasm_bytes,
        )
        request = internal_api_pb2.WriteFlagLogsRequest()
        request.telemetry_data.SetInParent()

        provider._write_logs(request.SerializeToString())
        decoded = internal_api_pb2.WriteFlagLogsRequest.FromString(
            provider._flag_logger.writes[0]
        )

        assert decoded.telemetry_data.sdk.id == types_pb2.SdkId.SDK_ID_PYTHON_PROVIDER
        assert decoded.telemetry_data.sdk.version == __version__
        assert len(decoded.telemetry_data.provider_init_rate) == 1

    def test_init_telemetry_retries_after_failed_write(
        self,
        wasm_bytes: bytes,
        test_client_secret: str,
    ) -> None:
        class FailOnceLogger(MockFlagLogger):
            def __init__(self) -> None:
                super().__init__()
                self.attempts = 0

            def write(self, request_bytes: bytes) -> None:
                self.attempts += 1
                if self.attempts == 1:
                    raise RuntimeError("send failed")
                super().write(request_bytes)

        mock_logger = FailOnceLogger()
        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
        )
        request = internal_api_pb2.WriteFlagLogsRequest()
        request.telemetry_data.SetInParent()
        encoded = request.SerializeToString()

        try:
            provider._write_logs(encoded)
        except RuntimeError:
            pass
        provider._write_logs(encoded)

        decoded = internal_api_pb2.WriteFlagLogsRequest.FromString(
            mock_logger.writes[0]
        )
        assert len(decoded.telemetry_data.provider_init_rate) == 1

    def test_flush_counted_failed_when_async_delivery_fails(
        self,
        wasm_bytes: bytes,
        test_client_secret: str,
    ) -> None:
        """Flush accounting must follow the real delivery, not the enqueue.

        Regression: write() only submits background work and the worker
        swallowed delivery failures, so flush_succeeded incremented for every
        flush while flush_failed and counter restoration never saw a
        network/HTTP failure.
        """
        from concurrent.futures import Future

        class FailingDeliveryLogger(MockFlagLogger):
            def write(self, request_bytes: bytes):  # type: ignore[override]
                super().write(request_bytes)
                future: "Future[bool]" = Future()
                future.set_result(False)  # enqueue OK, delivery failed
                return future

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            flag_logger=FailingDeliveryLogger(),
            wasm_bytes=wasm_bytes,
        )

        # Counters the flush will drain into the request.
        provider._flush_succeeded = 5
        provider._flush_failed = 0
        provider._event_telemetry_published = 40
        provider._event_telemetry_succeeded = 2
        provider._event_telemetry_failed = 1
        provider._event_telemetry_rejected = 3

        request = internal_api_pb2.WriteFlagLogsRequest()
        request.flag_assigned.add()
        provider._write_logs(request.SerializeToString())

        # The failed flush is counted, and every drained counter is restored so
        # the next flush re-reports it.
        assert provider._flush_failed >= 1, "delivery failure was not counted"
        assert provider._flush_succeeded == 5, "drained flush counter was not restored"
        assert provider._event_telemetry_published == 40
        assert provider._event_telemetry_succeeded == 2
        assert provider._event_telemetry_failed == 1
        assert provider._event_telemetry_rejected == 3

    def test_flush_counted_succeeded_when_async_delivery_succeeds(
        self,
        wasm_bytes: bytes,
        test_client_secret: str,
    ) -> None:
        """A delivered flush increments flush_succeeded and keeps counters drained."""
        from concurrent.futures import Future

        class SucceedingDeliveryLogger(MockFlagLogger):
            def write(self, request_bytes: bytes):  # type: ignore[override]
                super().write(request_bytes)
                future: "Future[bool]" = Future()
                future.set_result(True)
                return future

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            flag_logger=SucceedingDeliveryLogger(),
            wasm_bytes=wasm_bytes,
        )
        provider._flush_succeeded = 0
        provider._event_telemetry_published = 7

        request = internal_api_pb2.WriteFlagLogsRequest()
        request.flag_assigned.add()
        provider._write_logs(request.SerializeToString())

        assert provider._flush_succeeded == 1
        assert provider._event_telemetry_published == 0, (
            "counters were restored despite a successful delivery"
        )

    def test_assign_flush_is_counted_and_carries_drained_counters(
        self,
        wasm_bytes: bytes,
        test_client_secret: str,
    ) -> None:
        """Assign flushes are real WriteFlagLogs deliveries and must be counted.

        _flush_assigned previously called the logger directly and discarded the
        Future, so assign-interval batches were neither counted nor included in
        the host-counter drain. JS/Go/Java all count them.
        """
        from concurrent.futures import Future

        class SucceedingDeliveryLogger(MockFlagLogger):
            def write(self, request_bytes: bytes):  # type: ignore[override]
                super().write(request_bytes)
                future: "Future[bool]" = Future()
                future.set_result(True)
                return future

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            flag_logger=SucceedingDeliveryLogger(),
            wasm_bytes=wasm_bytes,
        )
        provider._flush_succeeded = 0
        provider._event_telemetry_published = 11

        request = internal_api_pb2.WriteFlagLogsRequest()
        request.flag_assigned.add()
        payload = request.SerializeToString()

        class StubResolver:
            def flush_assigned(self) -> bytes:
                return payload

        provider._resolver = StubResolver()  # type: ignore[assignment]

        provider._flush_assigned()

        assert provider._flush_succeeded == 1, (
            "the assign-flush delivery was never counted"
        )
        assert provider._event_telemetry_published == 0, (
            "assign flush did not carry the drained host counters"
        )

    def test_shutdown_drains_events_before_final_log_flush(
        self,
        wasm_bytes: bytes,
        test_client_secret: str,
    ) -> None:
        """Event outcomes ride on the next WriteFlagLogs.

        Draining events after the final flush strands the last batch's
        published/rejected/succeeded/failed counters in process-local state.
        Java already drains first.
        """
        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            flag_logger=MockFlagLogger(),
            wasm_bytes=wasm_bytes,
        )

        order: list[str] = []
        provider._event_tracker = object()  # type: ignore[assignment]
        provider._drain_events = lambda *a, **k: order.append("drain_events")  # type: ignore[assignment,method-assign]
        provider._write_logs = lambda *a, **k: order.append("write_logs")  # type: ignore[assignment,method-assign]

        class StubResolver:
            def flush_logs(self) -> bytes:
                return b"\x00"

        provider._resolver = StubResolver()  # type: ignore[assignment]

        provider.shutdown()

        assert "drain_events" in order, f"events were never drained; order={order}"
        assert "write_logs" in order, f"final logs were never flushed; order={order}"
        assert order.index("drain_events") < order.index("write_logs"), (
            f"events drained after the final log flush, so their counters are lost; order={order}"
        )

    def test_initialize_fetches_state(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """Test that initialize fetches state and sets up resolver."""
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
        )

        # Initialize should fetch state
        provider.initialize(EvaluationContext())

        # Verify state was fetched
        assert mock_fetcher.fetch_count == 1

        # Clean up
        provider.shutdown()


class TestResolveBoolean:
    """Tests for boolean flag resolution."""

    def test_resolve_boolean_returns_value(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """Test resolving a boolean flag returns correct value."""
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
        )

        provider.initialize(EvaluationContext())

        try:
            # Resolve a boolean flag (using tutorial-feature flag with path)
            ctx = EvaluationContext(
                targeting_key="test-user",
                attributes={"visitor_id": "tutorial_visitor"},
            )
            result = provider.resolve_boolean_details(
                flag_key="tutorial-feature.enabled",
                default_value=False,
                evaluation_context=ctx,
            )

            # Should return the flag's boolean value or default
            # Since tutorial-feature doesn't have 'enabled', should return default
            assert result.value is False
        finally:
            provider.shutdown()


class TestResolveString:
    """Tests for string flag resolution."""

    def test_resolve_string_returns_value(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """Test resolving a string flag returns correct value."""
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
        )

        provider.initialize(EvaluationContext())

        try:
            ctx = EvaluationContext(
                targeting_key="test-user",
                attributes={"visitor_id": "tutorial_visitor"},
            )
            result = provider.resolve_string_details(
                flag_key="tutorial-feature.message",
                default_value="default-message",
                evaluation_context=ctx,
            )

            # Should return the flag's message value
            expected = (
                "We are very excited to welcome you to Confidence! "
                "This is a message from the tutorial flag."
            )
            assert result.value == expected
            assert result.reason == Reason.TARGETING_MATCH
        finally:
            provider.shutdown()


class TestResolveInteger:
    """Tests for integer flag resolution."""

    def test_resolve_integer_returns_value(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """Test resolving an integer flag returns correct value."""
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
        )

        provider.initialize(EvaluationContext())

        try:
            ctx = EvaluationContext(
                targeting_key="test-user",
                attributes={"visitor_id": "tutorial_visitor"},
            )
            # Test with a path that doesn't exist - should return default
            result = provider.resolve_integer_details(
                flag_key="tutorial-feature.count",
                default_value=42,
                evaluation_context=ctx,
            )

            # Since count doesn't exist in tutorial-feature, should return default
            assert result.value == 42
        finally:
            provider.shutdown()

    def test_resolve_integer_accepts_whole_float(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """Whole floats should be accepted for integer resolution."""
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
        )

        provider.initialize(EvaluationContext())

        try:

            def fake_resolve_object(*args, **kwargs):
                return FlagResolutionDetails(value=2.0, reason=Reason.TARGETING_MATCH)

            provider._resolve_object = fake_resolve_object  # type: ignore[method-assign]

            result = provider.resolve_integer_details(
                flag_key="any-flag",
                default_value=7,
                evaluation_context=EvaluationContext(),
            )

            assert result.value == 2
            assert result.reason == Reason.TARGETING_MATCH
        finally:
            provider.shutdown()

    def test_resolve_integer_rejects_fractional_float(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """Fractional floats should be rejected for integer resolution."""
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
        )

        provider.initialize(EvaluationContext())

        try:

            def fake_resolve_object(*args, **kwargs):
                return FlagResolutionDetails(value=2.5, reason=Reason.TARGETING_MATCH)

            provider._resolve_object = fake_resolve_object  # type: ignore[method-assign]

            result = provider.resolve_integer_details(
                flag_key="any-flag",
                default_value=7,
                evaluation_context=EvaluationContext(),
            )

            assert result.value == 7
            assert result.reason == Reason.ERROR
            assert result.error_code == ErrorCode.TYPE_MISMATCH
        finally:
            provider.shutdown()


class TestResolveFloat:
    """Tests for float flag resolution."""

    def test_resolve_float_returns_value(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """Test resolving a float flag returns correct value."""
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
        )

        provider.initialize(EvaluationContext())

        try:
            ctx = EvaluationContext(
                targeting_key="test-user",
                attributes={"visitor_id": "tutorial_visitor"},
            )
            # Test with a path that doesn't exist - should return default
            result = provider.resolve_float_details(
                flag_key="tutorial-feature.ratio",
                default_value=3.14,
                evaluation_context=ctx,
            )

            # Since ratio doesn't exist in tutorial-feature, should return default
            assert result.value == 3.14
        finally:
            provider.shutdown()


class TestResolveObject:
    """Tests for object flag resolution."""

    def test_resolve_object_returns_value(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """Test resolving an object flag returns correct value."""
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
        )

        provider.initialize(EvaluationContext())

        try:
            ctx = EvaluationContext(
                targeting_key="test-user",
                attributes={"visitor_id": "tutorial_visitor"},
            )
            result = provider.resolve_object_details(
                flag_key="tutorial-feature",
                default_value={"message": "default"},
                evaluation_context=ctx,
            )

            # Should return the full flag value as object
            assert isinstance(result.value, dict)
            assert "message" in result.value
            expected_message = (
                "We are very excited to welcome you to Confidence! "
                "This is a message from the tutorial flag."
            )
            assert result.value["message"] == expected_message
            assert result.reason == Reason.TARGETING_MATCH
        finally:
            provider.shutdown()


class TestResolvePath:
    """Tests for nested path extraction."""

    def test_resolve_with_path(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """Test resolving a flag with nested path extraction."""
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
        )

        provider.initialize(EvaluationContext())

        try:
            ctx = EvaluationContext(
                targeting_key="test-user",
                attributes={"visitor_id": "tutorial_visitor"},
            )
            # tutorial-feature has title field
            result = provider.resolve_string_details(
                flag_key="tutorial-feature.title",
                default_value="default-title",
                evaluation_context=ctx,
            )

            assert result.value == "Welcome to Confidence!"
            assert result.reason == Reason.TARGETING_MATCH
        finally:
            provider.shutdown()

    def test_resolve_path_not_found(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """Test that non-existent path returns default value."""
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
        )

        provider.initialize(EvaluationContext())

        try:
            ctx = EvaluationContext(
                targeting_key="test-user",
                attributes={"visitor_id": "tutorial_visitor"},
            )
            result = provider.resolve_string_details(
                flag_key="tutorial-feature.nonexistent.path",
                default_value="default-value",
                evaluation_context=ctx,
            )

            # Should return default and flag not found error
            assert result.value == "default-value"
            assert result.reason == Reason.ERROR
            assert result.error_code == ErrorCode.FLAG_NOT_FOUND
        finally:
            provider.shutdown()


class TestResolveFlagNotFound:
    """Tests for flag not found scenarios."""

    def test_resolve_flag_not_found(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """Test that non-existent flag returns default value."""
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
        )

        provider.initialize(EvaluationContext())

        try:
            ctx = EvaluationContext(
                targeting_key="test-user",
                attributes={"visitor_id": "tutorial_visitor"},
            )
            result = provider.resolve_string_details(
                flag_key="nonexistent-flag",
                default_value="default-value",
                evaluation_context=ctx,
            )

            assert result.value == "default-value"
            assert result.reason == Reason.ERROR
            assert result.error_code == ErrorCode.FLAG_NOT_FOUND
        finally:
            provider.shutdown()


class TestResolveTypeMismatch:
    """Tests for type mismatch scenarios."""

    def test_string_as_boolean_returns_type_mismatch(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """Test that resolving a string as boolean returns type mismatch."""
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
        )

        provider.initialize(EvaluationContext())

        try:
            ctx = EvaluationContext(
                targeting_key="test-user",
                attributes={"visitor_id": "tutorial_visitor"},
            )
            # tutorial-feature.message is a string, try to resolve as boolean
            result = provider.resolve_boolean_details(
                flag_key="tutorial-feature.message",
                default_value=True,
                evaluation_context=ctx,
            )

            assert result.value is True
            assert result.reason == Reason.ERROR
            assert result.error_code == ErrorCode.TYPE_MISMATCH
            assert result.error_message == "Value is not bool"
        finally:
            provider.shutdown()

    def test_string_as_integer_returns_type_mismatch(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """Test that resolving a string as integer returns type mismatch."""
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
        )

        provider.initialize(EvaluationContext())

        try:
            ctx = EvaluationContext(
                targeting_key="test-user",
                attributes={"visitor_id": "tutorial_visitor"},
            )
            # tutorial-feature.message is a string, try to resolve as integer
            result = provider.resolve_integer_details(
                flag_key="tutorial-feature.message",
                default_value=42,
                evaluation_context=ctx,
            )

            assert result.value == 42
            assert result.reason == Reason.ERROR
            assert result.error_code == ErrorCode.TYPE_MISMATCH
            assert result.error_message == "Value is not int"
        finally:
            provider.shutdown()

    def test_string_as_float_returns_type_mismatch(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """Test that resolving a string as float returns type mismatch."""
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
        )

        provider.initialize(EvaluationContext())

        try:
            ctx = EvaluationContext(
                targeting_key="test-user",
                attributes={"visitor_id": "tutorial_visitor"},
            )
            # tutorial-feature.message is a string, try to resolve as float
            result = provider.resolve_float_details(
                flag_key="tutorial-feature.message",
                default_value=3.14,
                evaluation_context=ctx,
            )

            assert result.value == 3.14
            assert result.reason == Reason.ERROR
            assert result.error_code == ErrorCode.TYPE_MISMATCH
            assert result.error_message == "Value is not float"
        finally:
            provider.shutdown()

    def test_string_as_object_returns_type_mismatch(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """Test that resolving a string as object returns type mismatch."""
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
        )

        provider.initialize(EvaluationContext())

        try:
            ctx = EvaluationContext(
                targeting_key="test-user",
                attributes={"visitor_id": "tutorial_visitor"},
            )
            # tutorial-feature.message is a string, try to resolve as object
            default = {"key": "value"}
            result = provider.resolve_object_details(
                flag_key="tutorial-feature.message",
                default_value=default,
                evaluation_context=ctx,
            )

            assert result.value == default
            assert result.reason == Reason.ERROR
            assert result.error_code == ErrorCode.TYPE_MISMATCH
            assert result.error_message == "Value is not dict"
        finally:
            provider.shutdown()

    def test_object_as_string_returns_type_mismatch(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """Test that resolving an object as string returns type mismatch."""
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
        )

        provider.initialize(EvaluationContext())

        try:
            ctx = EvaluationContext(
                targeting_key="test-user",
                attributes={"visitor_id": "tutorial_visitor"},
            )
            # tutorial-feature is an object, try to resolve as string
            result = provider.resolve_string_details(
                flag_key="tutorial-feature",
                default_value="default",
                evaluation_context=ctx,
            )

            assert result.value == "default"
            assert result.reason == Reason.ERROR
            assert result.error_code == ErrorCode.TYPE_MISMATCH
            assert result.error_message == "Value is not str"
        finally:
            provider.shutdown()


class TestShutdown:
    """Tests for provider shutdown."""

    def test_shutdown_flushes_logs(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """Test that shutdown flushes pending logs."""
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
        )

        provider.initialize(EvaluationContext())

        # Resolve a flag to generate some logs
        ctx = EvaluationContext(
            targeting_key="test-user",
            attributes={"visitor_id": "tutorial_visitor"},
        )
        provider.resolve_string_details(
            flag_key="tutorial-feature.message",
            default_value="default",
            evaluation_context=ctx,
        )

        # Shutdown should flush logs
        provider.shutdown()

        # Verify shutdown was called on logger
        assert mock_logger.shutdown_called

    def test_shutdown_closes_materialization_store(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """Shutdown should close materialization store when supported."""

        class ClosableStore:
            def __init__(self) -> None:
                self.closed = False

            def read(self, ops):
                return []

            def write(self, ops) -> None:
                return None

            def close(self) -> None:
                self.closed = True

        store = ClosableStore()
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
            materialization_store=store,
        )

        provider.initialize(EvaluationContext())
        provider.shutdown()

        assert store.closed


class TestDefaultOnError:
    """Tests for error handling."""

    def test_returns_default_on_error(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
    ) -> None:
        """Test that provider returns default value on resolution error."""
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        # Use wrong client secret to trigger error
        provider = ConfidenceProvider(
            client_secret="wrong-secret",
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
        )

        provider.initialize(EvaluationContext())

        try:
            ctx = EvaluationContext(
                targeting_key="test-user",
                attributes={"visitor_id": "tutorial_visitor"},
            )
            result = provider.resolve_string_details(
                flag_key="tutorial-feature.message",
                default_value="default-value",
                evaluation_context=ctx,
            )

            assert result.value == "default-value"
            assert result.reason == Reason.ERROR
        finally:
            provider.shutdown()


class TestPrometheusMetrics:
    """Tests for Prometheus metrics export."""

    def test_get_prometheus_metrics(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """Test that get_prometheus_metrics returns metrics after resolution."""
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
        )

        provider.initialize(EvaluationContext())

        try:
            # Resolve a flag to generate telemetry
            ctx = EvaluationContext(
                targeting_key="test-user",
                attributes={"visitor_id": "tutorial_visitor"},
            )
            provider.resolve_string_details(
                flag_key="tutorial-feature.message",
                default_value="default-message",
                evaluation_context=ctx,
            )

            metrics = provider.get_prometheus_metrics()

            assert isinstance(metrics, str)
            assert len(metrics) > 0
            assert "confidence_resolve_latency" in metrics
        finally:
            provider.shutdown()


class TestDisableExposureCollection:
    """Tests for _confidence_skip_apply context key."""

    def test_resolve_with_disable_exposure_collection_still_resolves(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """Test that _confidence_skip_apply does not prevent resolution."""
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
        )

        provider.initialize(EvaluationContext())

        try:
            ctx = EvaluationContext(
                targeting_key="test-user",
                attributes={
                    "visitor_id": "tutorial_visitor",
                    "_confidence_skip_apply": True,
                },
            )
            result = provider.resolve_string_details(
                flag_key="tutorial-feature.message",
                default_value="default-message",
                evaluation_context=ctx,
            )

            assert result.reason == Reason.TARGETING_MATCH
            assert result.value != "default-message"
        finally:
            provider.shutdown()

    def test_disable_exposure_collection_config_still_resolves(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """disable_exposure_collection=True on the provider still resolves flags."""
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
            disable_exposure_collection=True,
        )

        provider.initialize(EvaluationContext())

        try:
            ctx = EvaluationContext(
                targeting_key="test-user",
                attributes={"visitor_id": "tutorial_visitor"},
            )
            result = provider.resolve_string_details(
                flag_key="tutorial-feature.message",
                default_value="default-message",
                evaluation_context=ctx,
            )
            assert result.reason == Reason.TARGETING_MATCH
            assert result.value != "default-message"
        finally:
            provider.shutdown()

        from confidence.proto.confidence.flags.resolver.v1 import internal_api_pb2

        assigned = 0
        client_resolve = 0
        flag_resolve = 0
        for payload in mock_logger.writes:
            req = internal_api_pb2.WriteFlagLogsRequest()
            req.ParseFromString(payload)
            assigned += len(req.flag_assigned)
            client_resolve += len(req.client_resolve_info)
            flag_resolve += len(req.flag_resolve_info)
        assert assigned == 0
        assert client_resolve >= 1
        assert flag_resolve >= 1

    def test_disable_exposure_collection_does_not_mutate_caller_context(
        self,
        wasm_bytes: bytes,
        test_resolver_state: bytes,
        test_account_id: str,
        test_client_secret: str,
    ) -> None:
        """Test that _confidence_skip_apply is left intact on the caller's context.

        The key must not be popped from the caller's attributes, otherwise a
        reused EvaluationContext would only skip apply on its first evaluation.
        """
        mock_fetcher = MockStateFetcher(test_resolver_state, test_account_id)
        mock_logger = MockFlagLogger()

        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=mock_fetcher,
            flag_logger=mock_logger,
            wasm_bytes=wasm_bytes,
        )

        provider.initialize(EvaluationContext())

        try:
            attrs = {
                "visitor_id": "tutorial_visitor",
                "_confidence_skip_apply": True,
            }
            ctx = EvaluationContext(
                targeting_key="test-user",
                attributes=attrs,
            )

            # Resolve twice with the same context to ensure disable_exposure_collection is
            # honored consistently and the key is never stripped.
            for _ in range(2):
                provider.resolve_string_details(
                    flag_key="tutorial-feature.message",
                    default_value="default-message",
                    evaluation_context=ctx,
                )
                assert attrs["_confidence_skip_apply"] is True
        finally:
            provider.shutdown()
