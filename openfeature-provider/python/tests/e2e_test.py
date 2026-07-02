"""End-to-end tests that verify flag resolution with the real backend."""

import os

import pytest
from openfeature import api
from openfeature.api import set_provider_and_wait
from openfeature.evaluation_context import EvaluationContext

from confidence import ConfidenceProvider

# E2E test configuration - matches Go e2e_test.go
E2E_CLIENT_SECRET = os.environ["CONFIDENCE_CLIENT_SECRET"]
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
            set_provider_and_wait(provider)
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
            set_provider_and_wait(provider)
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

    def test_resolve_without_materialization_store_uses_bloom_filter(self) -> None:
        """Without materialization store, bloom filters resolve membership locally."""
        provider = ConfidenceProvider(
            client_secret=E2E_CLIENT_SECRET,
            use_remote_materialization_store=False,
        )

        try:
            set_provider_and_wait(provider)
            client = api.get_client()

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
                f"Expected bloom filter resolve, got variant {result.variant}"
            )
        finally:
            provider.shutdown()


class TestFlagResolveWithEncryptedState:
    """E2E tests for flag resolution with encrypted CDN state."""

    def test_resolve_boolean_via_encrypted_state(self) -> None:
        encryption_key = os.environ.get("CONFIDENCE_CLIENT_ENCRYPTION_KEY")
        if not encryption_key:
            pytest.skip("CONFIDENCE_CLIENT_ENCRYPTION_KEY not set")

        provider = ConfidenceProvider(
            client_secret=E2E_CLIENT_SECRET,
            encryption_key=encryption_key,
        )
        try:
            set_provider_and_wait(provider)
            client = api.get_client()
            ctx = EvaluationContext(
                targeting_key="test-a",
                attributes={"sticky": False},
            )
            result = client.get_boolean_details(
                flag_key="web-sdk-e2e-flag.bool",
                default_value=True,
                evaluation_context=ctx,
            )
            assert result.value is False
        finally:
            provider.shutdown()

    def test_resolve_string_via_encrypted_state(self) -> None:
        encryption_key = os.environ.get("CONFIDENCE_CLIENT_ENCRYPTION_KEY")
        if not encryption_key:
            pytest.skip("CONFIDENCE_CLIENT_ENCRYPTION_KEY not set")

        provider = ConfidenceProvider(
            client_secret=E2E_CLIENT_SECRET,
            encryption_key=encryption_key,
        )
        try:
            set_provider_and_wait(provider)
            client = api.get_client()
            ctx = EvaluationContext(
                targeting_key="test-a",
                attributes={"sticky": False},
            )
            result = client.get_string_details(
                flag_key="web-sdk-e2e-flag.str",
                default_value="default",
                evaluation_context=ctx,
            )
            assert result.value == "control"
        finally:
            provider.shutdown()
