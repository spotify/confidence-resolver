package confidence

import (
	"io"
	"log/slog"
	"sync"
	"testing"
	"time"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
)

func newTestForwarder(t *testing.T, ingestion *testutil.MockIngestion) *LogForwarder {
	t.Helper()
	return NewLogForwarder(LogForwarderConfig{
		BaseURL:       ingestion.URL(),
		Logger:        testLogger(t),
		QueueDepth:    16,
		RetryAttempts: 5,
		RetryBackoff:  5 * time.Millisecond,
	})
}

func TestLogForwarderDeliversBatchWithToken(t *testing.T) {
	ingestion := testutil.NewMockIngestion(t)
	f := newTestForwarder(t, ingestion)
	defer f.Shutdown(time.Second)

	f.Enqueue([]byte("batch-body"), "token-1")

	if !ingestion.AwaitDelivered(t, 1, 2*time.Second) {
		t.Fatal("batch was not delivered")
	}
	if !awaitCondition(t, 2*time.Second, func() bool { return f.Stats().Sent == 1 }) {
		t.Fatalf("stats = %+v", f.Stats())
	}
	batches := ingestion.Batches()
	if len(batches) != 1 {
		t.Fatalf("ingested %d batches, want 1", len(batches))
	}
	if string(batches[0].Body) != "batch-body" {
		t.Errorf("body = %q", batches[0].Body)
	}
	if batches[0].Token != "Bearer token-1" {
		t.Errorf("authorization = %q", batches[0].Token)
	}
	if batches[0].ContentType != "application/x-protobuf" {
		t.Errorf("content type = %q", batches[0].ContentType)
	}
}

func TestLogForwarderBatchesCarryTheirOwnToken(t *testing.T) {
	ingestion := testutil.NewMockIngestion(t)
	f := newTestForwarder(t, ingestion)
	defer f.Shutdown(time.Second)

	// Two wrappers, two tokens: batches never share or re-group.
	f.Enqueue([]byte("body-1"), "token-1")
	f.Enqueue([]byte("body-2"), "token-2")

	if !ingestion.AwaitDelivered(t, 2, 2*time.Second) {
		t.Fatal("batches were not delivered")
	}
	batches := ingestion.Batches()
	if len(batches) != 2 {
		t.Fatalf("ingested %d batches, want 2", len(batches))
	}
	if batches[0].Token != "Bearer token-1" || batches[1].Token != "Bearer token-2" {
		t.Errorf("tokens %q / %q", batches[0].Token, batches[1].Token)
	}
}

func TestLogForwarderRetriesTransientFailures(t *testing.T) {
	ingestion := testutil.NewMockIngestion(t)
	f := newTestForwarder(t, ingestion)
	defer f.Shutdown(time.Second)

	ingestion.SetFail(503)
	f.Enqueue([]byte("body"), "token-1")

	// The retries (5 attempts, growing backoff) eventually land once the
	// failure clears.
	time.Sleep(10 * time.Millisecond)
	ingestion.SetFail(0)

	if !ingestion.AwaitDelivered(t, 1, 2*time.Second) {
		t.Fatal("batch was not delivered after failure cleared")
	}
	if !awaitCondition(t, 2*time.Second, func() bool { return f.Stats().Sent == 1 }) {
		t.Errorf("stats = %+v", f.Stats())
	}
}

func TestLogForwarderDropsAfterRetriesExhausted(t *testing.T) {
	ingestion := testutil.NewMockIngestion(t)
	f := newTestForwarder(t, ingestion)
	defer f.Shutdown(time.Second)

	ingestion.SetFail(500)
	f.Enqueue([]byte("body"), "token-1")

	if !awaitCondition(t, 2*time.Second, func() bool { return f.Stats().Dropped == 1 }) {
		t.Fatalf("batch was not dropped after retries exhausted: %+v", f.Stats())
	}
}

func TestLogForwarderDropsRejectedBatchesWithoutRetry(t *testing.T) {
	ingestion := testutil.NewMockIngestion(t)
	f := newTestForwarder(t, ingestion)
	defer f.Shutdown(time.Second)

	ingestion.SetFail(400)
	f.Enqueue([]byte("body"), "token-1")

	if !awaitCondition(t, 2*time.Second, func() bool {
		stats := f.Stats()
		return stats.Dropped == 1 && stats.Retried == 0
	}) {
		t.Fatalf("batch not dropped-and-counted: %+v", f.Stats())
	}
}

func TestLogForwarderFullQueueDrops(t *testing.T) {
	ingestion := testutil.NewMockIngestion(t)
	// Block ingestion so the queue fills.
	ingestion.SetFail(503)
	f := NewLogForwarder(LogForwarderConfig{
		BaseURL:       ingestion.URL(),
		Logger:        testLogger(t),
		QueueDepth:    2,
		RetryAttempts: 1,
		RetryBackoff:  time.Hour, // do not let retries spin during the test
	})
	defer f.Shutdown(time.Second)

	for i := 0; i < 8; i++ {
		f.Enqueue([]byte("body"), "token")
	}

	if !awaitCondition(t, 2*time.Second, func() bool { return f.Stats().Dropped > 0 }) {
		t.Fatal("overflow batches were not dropped")
	}
}

func TestLogForwarderConcurrentEnqueue(t *testing.T) {
	ingestion := testutil.NewMockIngestion(t)
	f := NewLogForwarder(LogForwarderConfig{
		BaseURL:       ingestion.URL(),
		Logger:        testLogger(t),
		QueueDepth:    64,
		RetryAttempts: 5,
		RetryBackoff:  5 * time.Millisecond,
	})
	defer f.Shutdown(time.Second)

	var wg sync.WaitGroup
	for i := 0; i < 32; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			f.Enqueue([]byte("body"), "token")
		}()
	}
	wg.Wait()

	if !ingestion.AwaitDelivered(t, 32, 3*time.Second) {
		stats := f.Stats()
		t.Fatalf("delivered %d, dropped %d", stats.Sent, stats.Dropped)
	}
}

// awaitCondition polls cond until it holds or the timeout passes.
func awaitCondition(t *testing.T, timeout time.Duration, cond func() bool) bool {
	t.Helper()
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		if cond() {
			return true
		}
		time.Sleep(2 * time.Millisecond)
	}
	return cond()
}

// testLogger returns a quiet logger for tests.
func testLogger(t *testing.T) *slog.Logger {
	return slog.New(slog.NewTextHandler(io.Discard, nil))
}
