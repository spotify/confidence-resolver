package event_tracking

import (
	"errors"
	"os"
	"testing"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/eventswasm"
	"google.golang.org/protobuf/types/known/timestamppb"
)

func loadTracker(t *testing.T) *EventTracker {
	t.Helper()
	wasmBytes, err := os.ReadFile("assets/confidence_event_engine.wasm")
	if err != nil {
		t.Fatalf("read embedded WASM: %v", err)
	}
	tracker, err := NewEventTracker(wasmBytes, false)
	if err != nil {
		t.Fatalf("NewEventTracker: %v", err)
	}
	t.Cleanup(func() { _ = tracker.Close() })
	return tracker
}

func TestTrackAndFlushAppliesEventDefinitionPrefix(t *testing.T) {
	tracker := loadTracker(t)

	err := tracker.TrackEvent(&eventswasm.TrackEventRequest{
		EventName: "my_event",
		EventTime: timestamppb.Now(),
	})
	if err != nil {
		t.Fatalf("TrackEvent: %v", err)
	}

	batch, err := tracker.FlushEvents()
	if err != nil {
		t.Fatalf("FlushEvents: %v", err)
	}
	if got := len(batch.GetEvents()); got != 1 {
		t.Fatalf("expected 1 event, got %d", got)
	}
	if got := batch.GetEvents()[0].GetEventDefinition(); got != "eventDefinitions/my_event" {
		t.Errorf("event_definition = %q, want %q", got, "eventDefinitions/my_event")
	}
}

func TestFlushIsIdempotentWhenEmpty(t *testing.T) {
	tracker := loadTracker(t)

	batch, err := tracker.FlushEvents()
	if err != nil {
		t.Fatalf("FlushEvents: %v", err)
	}
	if got := len(batch.GetEvents()); got != 0 {
		t.Errorf("expected empty batch, got %d events", got)
	}
}

// A reload discards every event buffered inside the instance, so only a genuine
// WASM trap or memory failure may trigger one. A client-side decode failure must
// leave the instance untouched.
func TestNonFatalErrorDoesNotReloadInstance(t *testing.T) {
	tracker := loadTracker(t)

	// Buffer a real event so the flush response carries bytes that cannot decode
	// as a TrackEventRequest (field 1 there is a UTF-8 string, here it is a
	// nested Event message). An empty response would decode into either type.
	if err := tracker.TrackEvent(&eventswasm.TrackEventRequest{
		EventName: "decode_mismatch_probe",
		EventTime: timestamppb.Now(),
	}); err != nil {
		t.Fatalf("TrackEvent: %v", err)
	}

	before := tracker.instance

	// Decoding the guest's FlushEventsResponse bytes into an unrelated message
	// fails client-side. The instance itself is perfectly healthy.
	err := tracker.call("wasm_msg_guest_bounded_flush_events", nil, &eventswasm.TrackEventRequest{})
	if err == nil {
		t.Fatal("expected a decode error when reading the flush response as the wrong type")
	}
	if errors.Is(err, errWasmFatal) {
		t.Fatalf("a decode failure must not be classified fatal, got %v", err)
	}
	if tracker.instance != before {
		t.Error("instance was reloaded on a non-fatal error, discarding buffered events")
	}
}

// A missing export means the module is not what we expect: that is fatal and
// must reload.
func TestFatalErrorReloadsInstance(t *testing.T) {
	tracker := loadTracker(t)
	before := tracker.instance

	err := tracker.call("wasm_msg_guest_does_not_exist", nil, nil)
	if !errors.Is(err, errWasmFatal) {
		t.Fatalf("expected a fatal error, got %v", err)
	}
	if tracker.instance == before {
		t.Error("instance was not reloaded after a fatal error")
	}
}

// TestFatalErrorReloadsToFunctionalInstance verifies that the event tracker is
// fully functional both before and after a WASM crash. The previous test
// (TestFatalErrorReloadsInstance) only proves the instance pointer changes; this
// test proves the new instance can actually track and flush events.
func TestFatalErrorReloadsToFunctionalInstance(t *testing.T) {
	tracker := loadTracker(t)

	// ── Before crash: track + flush and verify the event comes through ──
	if err := tracker.TrackEvent(&eventswasm.TrackEventRequest{
		EventName: "before_crash",
		EventTime: timestamppb.Now(),
	}); err != nil {
		t.Fatalf("TrackEvent (before crash): %v", err)
	}

	batch, err := tracker.FlushEvents()
	if err != nil {
		t.Fatalf("FlushEvents (before crash): %v", err)
	}
	if got := len(batch.GetEvents()); got != 1 {
		t.Fatalf("before crash: expected 1 event, got %d", got)
	}
	if got := batch.GetEvents()[0].GetEventDefinition(); got != "eventDefinitions/before_crash" {
		t.Errorf("before crash: event_definition = %q, want %q", got, "eventDefinitions/before_crash")
	}

	// ── Trigger a crash (call a non-existent export) ──
	err = tracker.call("wasm_msg_guest_does_not_exist", nil, nil)
	if !errors.Is(err, errWasmFatal) {
		t.Fatalf("expected a fatal error, got %v", err)
	}

	// ── After crash: track + flush and verify the new instance works ──
	if err := tracker.TrackEvent(&eventswasm.TrackEventRequest{
		EventName: "after_crash",
		EventTime: timestamppb.Now(),
	}); err != nil {
		t.Fatalf("TrackEvent (after crash): %v", err)
	}

	batch, err = tracker.FlushEvents()
	if err != nil {
		t.Fatalf("FlushEvents (after crash): %v", err)
	}
	if got := len(batch.GetEvents()); got != 1 {
		t.Fatalf("after crash: expected 1 event, got %d", got)
	}
	if got := batch.GetEvents()[0].GetEventDefinition(); got != "eventDefinitions/after_crash" {
		t.Errorf("after crash: event_definition = %q, want %q", got, "eventDefinitions/after_crash")
	}
}

func TestClosedTrackerRejectsCalls(t *testing.T) {
	tracker := loadTracker(t)
	if err := tracker.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}
	if err := tracker.TrackEvent(&eventswasm.TrackEventRequest{EventName: "x"}); err == nil {
		t.Error("expected an error after Close")
	}
}
