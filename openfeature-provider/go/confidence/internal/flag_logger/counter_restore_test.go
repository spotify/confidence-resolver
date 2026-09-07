package flag_logger

import (
	"testing"

	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
)

// TestCounterRestoreOnFailure verifies that after N consecutive failures
// followed by 1 success, the backend sees flush_failed=N (not 1), and that the
// event counters drained by each failed attempt survive to the successful one.
//
// This drives the real TelemetryCounters — DrainAndStamp and RestoreOnFailure —
// rather than a local reimplementation of them, so gutting either method fails
// this test.
func TestCounterRestoreOnFailure(t *testing.T) {
	var tc TelemetryCounters

	// Event activity accumulated before any flush is attempted.
	tc.RecordEventBatch(500, 7, true) // 500 ingested, 7 refused, 1 batch OK
	tc.RecordEventBatch(0, 0, false)  // 1 batch failed outright

	// Three consecutive failed deliveries. Each drains onto its request and
	// then restores, so nothing is lost and each failure is counted once.
	const failures = 3
	for i := 0; i < failures; i++ {
		request := &resolverv1.WriteFlagLogsRequest{}
		tc.DrainAndStamp(request)
		tc.RestoreOnFailure(request)
	}

	// Fourth delivery succeeds: this request is what the backend actually sees.
	delivered := &resolverv1.WriteFlagLogsRequest{}
	tc.DrainAndStamp(delivered)
	tc.FlushSucceeded.Add(1) // the success is recorded for the NEXT flush

	if delivered.TelemetryData == nil {
		t.Fatal("telemetry data should be stamped onto the delivered request")
	}
	if delivered.TelemetryData.Flush == nil {
		t.Fatal("flush telemetry should be present")
	}
	if got := delivered.TelemetryData.Flush.Failed; got != failures {
		t.Errorf("flush.failed: got %d, want %d — each failed attempt must be counted once", got, failures)
	}
	if got := delivered.TelemetryData.Flush.Succeeded; got != 0 {
		t.Errorf("flush.succeeded: got %d, want 0 — the success is reported by the next flush", got)
	}

	if delivered.TelemetryData.Events == nil {
		t.Fatal("events telemetry should be present — it was drained by the failed attempts")
	}
	ev := delivered.TelemetryData.Events
	if ev.Published != 500 {
		t.Errorf("events.published: got %d, want 500 — lost across the failed attempts", ev.Published)
	}
	if ev.BatchesSucceeded != 1 {
		t.Errorf("events.batches_succeeded: got %d, want 1", ev.BatchesSucceeded)
	}
	if ev.BatchesFailed != 1 {
		t.Errorf("events.batches_failed: got %d, want 1", ev.BatchesFailed)
	}
	if ev.EventsRejected != 7 {
		t.Errorf("events.events_rejected: got %d, want 7 — lost across the failed attempts", ev.EventsRejected)
	}

	// The successful delivery drained everything, leaving only the new success.
	if v := tc.FlushSucceeded.Load(); v != 1 {
		t.Errorf("post-success FlushSucceeded: got %d, want 1", v)
	}
	if v := tc.FlushFailed.Load(); v != 0 {
		t.Errorf("post-success FlushFailed: got %d, want 0", v)
	}
	if v := tc.EventsPublished.Load(); v != 0 {
		t.Errorf("post-success EventsPublished: got %d, want 0", v)
	}
	if v := tc.EventsRejected.Load(); v != 0 {
		t.Errorf("post-success EventsRejected: got %d, want 0", v)
	}
}

// TestRecordEventBatchTracksRejections exercises the real TelemetryCounters to
// verify a partially-rejected batch reports published net of rejections while
// still counting the rejections, and that a drained-then-failed request
// restores them.
func TestRecordEventBatchTracksRejections(t *testing.T) {
	var tc TelemetryCounters

	// A batch of 50 with 3 refused: 47 ingested, 3 rejected, 1 batch OK.
	tc.RecordEventBatch(47, 3, true)

	request := &resolverv1.WriteFlagLogsRequest{}
	tc.DrainAndStamp(request)

	if request.TelemetryData == nil || request.TelemetryData.Events == nil {
		t.Fatal("events telemetry should be stamped onto the request")
	}
	ev := request.TelemetryData.Events
	if ev.Published != 47 {
		t.Errorf("published: got %d, want 47", ev.Published)
	}
	if ev.EventsRejected != 3 {
		t.Errorf("events_rejected: got %d, want 3", ev.EventsRejected)
	}
	if ev.BatchesSucceeded != 1 {
		t.Errorf("batches_succeeded: got %d, want 1", ev.BatchesSucceeded)
	}

	// Drain emptied the counters.
	if v := tc.EventsRejected.Load(); v != 0 {
		t.Errorf("post-drain EventsRejected: got %d, want 0", v)
	}

	// A failed send must put the rejections back for the next flush.
	tc.RestoreOnFailure(request)
	if v := tc.EventsRejected.Load(); v != 3 {
		t.Errorf("post-restore EventsRejected: got %d, want 3", v)
	}

	// A failed batch records no publications or rejections.
	var tc2 TelemetryCounters
	tc2.RecordEventBatch(10, 0, false)
	if v := tc2.EventsPublished.Load(); v != 0 {
		t.Errorf("failed batch EventsPublished: got %d, want 0", v)
	}
	if v := tc2.EventsRejected.Load(); v != 0 {
		t.Errorf("failed batch EventsRejected: got %d, want 0", v)
	}
	if v := tc2.EventBatchesFailed.Load(); v != 1 {
		t.Errorf("failed batch EventBatchesFailed: got %d, want 1", v)
	}
}
