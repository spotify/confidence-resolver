"""End-to-end tests that verify flag logging with the real backend.

These tests require a valid client secret and network connectivity.
Skip these tests in CI by running: pytest --ignore=tests/flaglogs_e2e_test.py
"""

import os
import time

import pytest
from openfeature import api
from openfeature.evaluation_context import EvaluationContext

from confidence_openfeature import ConfidenceProvider

# E2E test configuration
E2E_CLIENT_SECRET = os.environ.get(
    "CONFIDENCE_CLIENT_SECRET",
    "Ip7lGcBeGA4Le9MI8md4i5LkUOnLnyFx",
)

# Skip all tests if no client secret is available
pytestmark = pytest.mark.skipif(
    not E2E_CLIENT_SECRET,
    reason="CONFIDENCE_CLIENT_SECRET environment variable not set",
)


class TestFlagLogging:
    """E2E tests for flag logging to the backend."""

    def test_flag_logs_sent_on_shutdown(self) -> None:
        """Flag logs are successfully sent to the backend on shutdown."""
        provider = ConfidenceProvider(
            client_secret=E2E_CLIENT_SECRET,
            log_poll_interval=1.0,  # Short interval for testing
        )

        try:
            provider.initialize(EvaluationContext())
            api.set_provider(provider)
            client = api.get_client()

            # Perform multiple resolves to generate logs
            for i in range(5):
                ctx = EvaluationContext(
                    targeting_key=f"flaglog-test-user-{i}",
                    attributes={"visitor_id": "tutorial_visitor"},
                )

                client.get_string_value(
                    flag_key="tutorial-feature.message",
                    default_value="default message",
                    evaluation_context=ctx,
                )

            # Wait for log flush
            time.sleep(2.0)

        finally:
            # Shutdown should flush remaining logs
            provider.shutdown()

    def test_flag_logs_sent_periodically(self) -> None:
        """Flag logs are sent periodically during normal operation."""
        provider = ConfidenceProvider(
            client_secret=E2E_CLIENT_SECRET,
            log_poll_interval=1.0,  # Short interval for testing
        )

        try:
            provider.initialize(EvaluationContext())
            api.set_provider(provider)
            client = api.get_client()

            # Resolve a flag
            ctx = EvaluationContext(
                targeting_key="flaglog-periodic-test-user",
                attributes={"visitor_id": "tutorial_visitor"},
            )

            client.get_string_value(
                flag_key="tutorial-feature.message",
                default_value="default message",
                evaluation_context=ctx,
            )

            # Wait for periodic log flush
            time.sleep(2.0)

            # Resolve another flag after the flush
            ctx2 = EvaluationContext(
                targeting_key="flaglog-periodic-test-user-2",
                attributes={"visitor_id": "tutorial_visitor"},
            )

            client.get_string_value(
                flag_key="tutorial-feature.title",
                default_value="default title",
                evaluation_context=ctx2,
            )

        finally:
            provider.shutdown()


class TestAssignmentLogging:
    """E2E tests for assignment logging."""

    def test_assignments_logged_for_resolved_flags(self) -> None:
        """Flag assignments are logged when flags are resolved."""
        provider = ConfidenceProvider(
            client_secret=E2E_CLIENT_SECRET,
            assign_poll_interval=0.1,  # Fast assignment polling
        )

        try:
            provider.initialize(EvaluationContext())
            api.set_provider(provider)
            client = api.get_client()

            ctx = EvaluationContext(
                targeting_key="assignment-log-test-user",
                attributes={"visitor_id": "tutorial_visitor"},
            )

            # Resolve multiple flag types
            client.get_string_value(
                flag_key="tutorial-feature.message",
                default_value="default message",
                evaluation_context=ctx,
            )

            client.get_string_value(
                flag_key="tutorial-feature.title",
                default_value="default title",
                evaluation_context=ctx,
            )

            # Wait for assignment logs to be flushed
            time.sleep(0.5)

        finally:
            provider.shutdown()
