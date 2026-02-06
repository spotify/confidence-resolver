"""Tests for ConfidenceProvider class."""

from openfeature.evaluation_context import EvaluationContext
from openfeature.exception import ErrorCode
from openfeature.flag_evaluation import FlagResolutionDetails, Reason

from confidence.provider import ConfidenceProvider
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
