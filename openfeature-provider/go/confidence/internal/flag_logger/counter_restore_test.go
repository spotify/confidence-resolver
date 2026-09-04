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

	drain := func() *resolverv1.TelemetryData {
		fs := uint32(flushSucceeded.Swap(0))
		ff := uint32(flushFailed.Swap(0))
		ep := uint32(eventsPublished.Swap(0))
		ebs := uint32(eventBatchesSucceeded.Swap(0))
		ebf := uint32(eventBatchesFailed.Swap(0))
		td := &resolverv1.TelemetryData{}
		if fs > 0 || ff > 0 {
			td.Flush = &resolverv1.TelemetryData_FlushTelemetry{Succeeded: fs, Failed: ff}
		}
		if ep > 0 || ebs > 0 || ebf > 0 {
			td.Events = &resolverv1.TelemetryData_EventsTelemetry{
				Published: ep, BatchesSucceeded: ebs, BatchesFailed: ebf,
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
		}
	}

	recordSuccess := func() {
		flushSucceeded.Add(1)
	}

	// Simulate some event activity before flushes start
	eventsPublished.Add(500)
	eventBatchesSucceeded.Add(3)
	eventBatchesFailed.Add(1)

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

	// After success: only the new success should be in the atomics
	if v := flushSucceeded.Load(); v != 1 {
		t.Errorf("post-success flushSucceeded: got %d, want 1", v)
	}
	if v := flushFailed.Load(); v != 0 {
		t.Errorf("post-success flushFailed: got %d, want 0", v)
	}
}
