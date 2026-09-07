package flag_logger

import (
	"sync/atomic"
	"testing"

	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
)

// TestCounterRestoreOnFailure verifies that after N consecutive failures
// followed by 1 success, the backend sees flush_failed=N (not 1).
func TestCounterRestoreOnFailure(t *testing.T) {
	// Simulate the counter drain → set on request → failure → restore cycle
	// using raw atomics (same logic as GrpcFlagLogger/MultiDestinationFlagLogger).

	var flushSucceeded, flushFailed atomic.Int64
	var eventsPublished, eventBatchesSucceeded, eventBatchesFailed atomic.Int64
	var eventsRejected atomic.Int64

	drain := func() *resolverv1.TelemetryData {
		fs := uint32(flushSucceeded.Swap(0))
		ff := uint32(flushFailed.Swap(0))
		ep := uint32(eventsPublished.Swap(0))
		ebs := uint32(eventBatchesSucceeded.Swap(0))
		ebf := uint32(eventBatchesFailed.Swap(0))
		er := uint32(eventsRejected.Swap(0))
		td := &resolverv1.TelemetryData{}
		if fs > 0 || ff > 0 {
			td.Flush = &resolverv1.TelemetryData_FlushTelemetry{Succeeded: fs, Failed: ff}
		}
		if ep > 0 || ebs > 0 || ebf > 0 || er > 0 {
			td.Events = &resolverv1.TelemetryData_EventsTelemetry{
				Published: ep, BatchesSucceeded: ebs, BatchesFailed: ebf, EventsRejected: er,
			}
		}
		return td
	}

	restoreOnFailure := func(td *resolverv1.TelemetryData) {
		flushFailed.Add(1) // record this failure
		if td.Flush != nil {
			flushSucceeded.Add(int64(td.Flush.Succeeded))
			flushFailed.Add(int64(td.Flush.Failed))
		}
		if td.Events != nil {
			eventsPublished.Add(int64(td.Events.Published))
			eventBatchesSucceeded.Add(int64(td.Events.BatchesSucceeded))
			eventBatchesFailed.Add(int64(td.Events.BatchesFailed))
			eventsRejected.Add(int64(td.Events.EventsRejected))
		}
	}

	recordSuccess := func() {
		flushSucceeded.Add(1)
	}

	// Simulate some event activity before flushes start
	eventsPublished.Add(500)
	eventBatchesSucceeded.Add(3)
	eventBatchesFailed.Add(1)
	eventsRejected.Add(7)

	// Flush 1: drain → FAIL
	td1 := drain()
	restoreOnFailure(td1)
	// After: flushFailed=1, eventsPublished=500, eventBatchesSucceeded=3, eventBatchesFailed=1

	// Flush 2: drain → FAIL
	td2 := drain()
	restoreOnFailure(td2)
	// After: flushFailed=2 (1 restored + 1 new), events restored

	// Flush 3: drain → FAIL
	td3 := drain()
	restoreOnFailure(td3)
	// After: flushFailed=3

	// Flush 4: drain → SUCCESS
	td4 := drain()
	recordSuccess()

	// Verify: the backend (td4) should see flush_failed=3 and all event counters
	if td4.Flush == nil {
		t.Fatal("flush telemetry should be present")
	}
	if td4.Flush.Failed != 3 {
		t.Errorf("flush.failed: got %d, want 3", td4.Flush.Failed)
	}
	if td4.Flush.Succeeded != 0 {
		t.Errorf("flush.succeeded: got %d, want 0", td4.Flush.Succeeded)
	}

	if td4.Events == nil {
		t.Fatal("events telemetry should be present")
	}
	if td4.Events.Published != 500 {
		t.Errorf("events.published: got %d, want 500", td4.Events.Published)
	}
	if td4.Events.BatchesSucceeded != 3 {
		t.Errorf("events.batches_succeeded: got %d, want 3", td4.Events.BatchesSucceeded)
	}
	if td4.Events.BatchesFailed != 1 {
		t.Errorf("events.batches_failed: got %d, want 1", td4.Events.BatchesFailed)
	}
	if td4.Events.EventsRejected != 7 {
		t.Errorf("events.events_rejected: got %d, want 7", td4.Events.EventsRejected)
	}

	// After success: only the new success should be in the atomics
	if v := flushSucceeded.Load(); v != 1 {
		t.Errorf("post-success flushSucceeded: got %d, want 1", v)
	}
	if v := flushFailed.Load(); v != 0 {
		t.Errorf("post-success flushFailed: got %d, want 0", v)
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
