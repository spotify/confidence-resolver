package confidence

import (
	"context"
	"strings"
	"sync"
	"testing"
	"time"
)

// slowMaterializationStore wraps a store, delaying every read so resolves
// stay suspended (and, since suspensions hold no pool instance, only a
// read lock) while shutdown lands.
type slowMaterializationStore struct {
	inner MaterializationStore
	delay time.Duration
}

func (s *slowMaterializationStore) Read(ctx context.Context, ops []ReadOp) ([]ReadResult, error) {
	select {
	case <-time.After(s.delay):
	case <-ctx.Done():
		return nil, ctx.Err()
	}
	return s.inner.Read(ctx, ops)
}

func (s *slowMaterializationStore) Write(ctx context.Context, ops []WriteOp) error {
	return s.inner.Write(ctx, ops)
}

// TestShutdownWithInFlightSuspendedWork evaluates suspending flags from
// many goroutines against a slow materialization store and shuts the
// provider down while those resolves are mid-flight in their store reads.
// Shutdown must drain them — every in-flight resolve completes with its
// correct value — and the provider must end closed and keep answering
// from then on with defaults instead of panicking or deadlocking.
func TestShutdownWithInFlightSuspendedWork(t *testing.T) {
	store := &slowMaterializationStore{
		inner: newInMemoryMaterializationStoreWithInclusions(testLogger(t), map[string]map[string]bool{
			"u1": {"materializedSegments/nicklas-custom-targeting": true},
		}),
		delay: 40 * time.Millisecond,
	}
	provider := newE2EProvider(t, store)

	const evaluators = 8
	const iterations = 6
	var wg sync.WaitGroup
	results := make([][]string, evaluators)
	for g := 0; g < evaluators; g++ {
		wg.Add(1)
		go func(g int) {
			defer wg.Done()
			for i := 0; i < iterations; i++ {
				detail := provider.StringEvaluation(context.Background(), "custom-targeted-flag.message", "fallback", cakeEvalCtx())
				results[g] = append(results[g], detail.Value)
			}
		}(g)
	}

	// Let some resolves reach their suspension windows, then race shutdown
	// against the rest.
	time.Sleep(100 * time.Millisecond)
	provider.Shutdown()
	wg.Wait()

	for g, values := range results {
		for i, v := range values {
			// During the drain the value is the cake variant; after the
			// close it is the echoed default. Anything else is a bug.
			if v != "Did someone say CAKE?!" && v != "fallback" {
				t.Errorf("goroutine %d iteration %d: value = %q", g, i, v)
			}
		}
	}

	// The provider is closed: further evaluations return defaults, not
	// crashes.
	detail := provider.StringEvaluation(context.Background(), "custom-targeted-flag.message", "fallback", cakeEvalCtx())
	if detail.Value != "fallback" {
		t.Errorf("evaluation after shutdown = %q, want the default", detail.Value)
	}
}

// TestShutdownWithInFlightEvaluations races plain (non-suspending)
// evaluations against shutdown: the read-lock discipline lets in-flight
// calls finish before the host is destroyed, and calls that arrive after
// the close see defaults.
func TestShutdownWithInFlightEvaluations(t *testing.T) {
	provider, _ := newTestProvider(t)

	var wg sync.WaitGroup
	for g := 0; g < 8; g++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for i := 0; i < 25; i++ {
				detail := provider.StringEvaluation(context.Background(), "tutorial-feature.message", "fallback", tutorialEvalCtx())
				// Either the live value (before close) or the default
				// (after): both are clean outcomes.
				if detail.Value != "fallback" && !strings.HasPrefix(detail.Value, "We are very excited") {
					t.Errorf("evaluation during shutdown = %q", detail.Value)
					return
				}
			}
		}()
	}
	provider.Shutdown()
	wg.Wait()

	detail := provider.StringEvaluation(context.Background(), "tutorial-feature.message", "fallback", tutorialEvalCtx())
	if detail.Value != "fallback" {
		t.Errorf("evaluation after shutdown = %q, want the default", detail.Value)
	}
}
