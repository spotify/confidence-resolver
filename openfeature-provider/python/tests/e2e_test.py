"""End-to-end tests that verify flag resolution with the real backend."""

from openfeature import api
from openfeature.evaluation_context import EvaluationContext

from confidence_openfeature import ConfidenceProvider

# E2E test configuration - matches Go e2e_test.go
E2E_CLIENT_SECRET = "Ip7lGcBeGA4Le9MI8md4i5LkUOnLnyFx"
E2E_INCLUDED_TARGETING_KEY = "user-a"
E2E_EXCLUDED_TARGETING_KEY = "user-x"


class TestFlagResolveWithRemoteMaterializationStore:
    """E2E tests for flag resolution with remote materialization store."""

    def test_resolve_included_user_gets_treatment(self) -> None:
        """User in materialized segment gets treatment variant."""
        provider = ConfidenceProvider(
            client_secret=E2E_CLIENT_SECRET,
            use_remote_materialization_store=True,
        )

        try:
            provider.initialize(EvaluationContext())
            api.set_provider(provider)
            client = api.get_client()

            # Use targetless context with only user_id, matching Go tests
            ctx = EvaluationContext(
                attributes={"user_id": E2E_INCLUDED_TARGETING_KEY},
            )

            result = client.get_string_details(
                flag_key="custom-targeted-flag.message",
                default_value="client default",
                evaluation_context=ctx,
            )

            expected = "flags/custom-targeted-flag/variants/cake-exclamation"
            assert result.variant == expected, (
                f"Expected cake-exclamation variant, got {result.variant}, "
                f"error: {result.error_message}"
            )
        finally:
            provider.shutdown()

    def test_resolve_excluded_user_gets_default(self) -> None:
        """User not in materialized segment gets default variant."""
        provider = ConfidenceProvider(
            client_secret=E2E_CLIENT_SECRET,
            use_remote_materialization_store=True,
        )

        try:
            provider.initialize(EvaluationContext())
            api.set_provider(provider)
            client = api.get_client()

            # Use targetless context with only user_id, matching Go tests
            ctx = EvaluationContext(
                attributes={"user_id": E2E_EXCLUDED_TARGETING_KEY},
            )

            result = client.get_string_details(
                flag_key="custom-targeted-flag.message",
                default_value="client default",
                evaluation_context=ctx,
            )

            expected = "flags/custom-targeted-flag/variants/default"
            assert result.variant == expected, (
                f"Expected default variant, got {result.variant}, "
                f"error: {result.error_message}"
            )
        finally:
            provider.shutdown()


class TestFlagResolveWithoutMaterializationStore:
    """E2E tests for flag resolution without materialization store."""

    def test_resolve_without_materialization_returns_error(self) -> None:
        """Without materialization store, flags needing it return error."""
        provider = ConfidenceProvider(
            client_secret=E2E_CLIENT_SECRET,
            use_remote_materialization_store=False,
        )

        try:
            provider.initialize(EvaluationContext())
            api.set_provider(provider)
            client = api.get_client()

            ctx = EvaluationContext(
                attributes={"user_id": E2E_INCLUDED_TARGETING_KEY},
            )

            result = client.get_string_details(
                flag_key="custom-targeted-flag.message",
                default_value="client default",
                evaluation_context=ctx,
            )

            # Without materialization store, should return default with error
            assert result.value == "client default"
            assert "materialization" in (result.error_message or "").lower()
        finally:
            provider.shutdown()
