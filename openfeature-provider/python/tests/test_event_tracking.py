"""Tests for event tracking: interface conformance and error semantics.

Python's duck typing lets a provider declare a track() that does not match the
OpenFeature interface and still import cleanly — the client then never routes to
it. Go catches this with a compile-time interface assertion and Java with
@Override; these tests are the Python equivalent.
"""

import inspect

import pytest
from openfeature.provider import AbstractProvider

from confidence.event_tracker import EventEngineError, EventTracker, WasmCrashError
from confidence.provider import ConfidenceProvider
from confidence.proto.confidence.events.wasm.v1 import wasm_api_pb2
from tests.conftest import MockFlagLogger, MockStateFetcher


class TestTrackInterfaceConformance:
    """ConfidenceProvider.track must match the OpenFeature provider interface."""

    def test_track_signature_matches_openfeature_interface(self) -> None:
        expected = inspect.signature(AbstractProvider.track)
        actual = inspect.signature(ConfidenceProvider.track)

        assert list(actual.parameters) == list(expected.parameters), (
            "track() parameter names must match the OpenFeature interface; "
            f"expected {list(expected.parameters)}, got {list(actual.parameters)}"
        )

    def test_track_accepts_the_documented_call_shape(
        self,
        wasm_bytes: bytes,
        test_client_secret: str,
    ) -> None:
        """Calling track() the way the OpenFeature client does must not raise."""
        provider = ConfidenceProvider(
            client_secret=test_client_secret,
            state_fetcher=MockStateFetcher(b"", "acct"),
            flag_logger=MockFlagLogger(),
            wasm_bytes=wasm_bytes,
        )
        # Event tracking not configured, so this is a documented no-op.
        provider.track("purchase")
        provider.track("purchase", None, None)


class TestEventTrackerErrorSemantics:
    """A reload discards the instance's buffered events, so it must be narrow."""

    def test_guest_reported_error_is_not_treated_as_a_crash(self) -> None:
        # Regression: WasmCrashError used to include RuntimeError, and a clean
        # guest error envelope raises one — so a healthy instance got rebuilt and
        # its buffered events discarded.
        assert not isinstance(EventEngineError("boom"), WasmCrashError), (
            "EventEngineError must not match WasmCrashError, or a guest-reported "
            "error will trigger a reload and drop buffered events"
        )

    def test_event_engine_error_is_a_runtime_error(self) -> None:
        # Subclassing RuntimeError keeps existing `except RuntimeError` callers working.
        assert issubclass(EventEngineError, RuntimeError)

    def test_track_and_flush_applies_event_definition_prefix(
        self, event_wasm_bytes: bytes
    ) -> None:
        tracker = EventTracker(event_wasm_bytes)
        tracker.track_event(wasm_api_pb2.TrackEventRequest(event_name="my_event"))

        batch = tracker.flush_events()
        assert len(batch.events) == 1
        assert batch.events[0].event_definition == "eventDefinitions/my_event"

    def test_flush_drains_the_buffer(self, event_wasm_bytes: bytes) -> None:
        tracker = EventTracker(event_wasm_bytes)
        tracker.track_event(wasm_api_pb2.TrackEventRequest(event_name="once"))

        assert len(tracker.flush_events().events) == 1
        assert len(tracker.flush_events().events) == 0

    def test_explicit_zero_value_is_preserved(self, event_wasm_bytes: bytes) -> None:
        # Python's TrackingEventDetails.value is Optional[float], so unlike Go it
        # can distinguish an explicit 0 from "not set".
        tracker = EventTracker(event_wasm_bytes)
        tracker.track_event(
            wasm_api_pb2.TrackEventRequest(event_name="zero", value=0.0)
        )

        batch = tracker.flush_events()
        assert len(batch.events) == 1
        assert batch.events[0].payload.fields["value"].number_value == 0.0


@pytest.fixture
def event_wasm_bytes() -> bytes:
    """The compiled event engine WASM, mirroring conftest's wasm_bytes fixture."""
    from pathlib import Path

    path = (
        Path(__file__).parent.parent
        / "resources"
        / "wasm"
        / "confidence_event_engine.wasm"
    )
    if not path.exists():
        pytest.skip("event engine WASM not found at {}".format(path))
    return path.read_bytes()
