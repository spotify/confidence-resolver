package local_resolver

import (
	"context"
	"sync"
	"sync/atomic"
	"testing"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
)

// mockResolver is a controllable LocalResolver where panicking can
// be toggled per-instance via the shouldPanic field.
type mockResolver struct {
	shouldPanic bool
}

func (m *mockResolver) SetResolverState(*wasm.SetResolverStateRequest) error { return nil }
func (m *mockResolver) ResolveProcess(*wasm.ResolveProcessRequest) (*wasm.ResolveProcessResponse, error) {
	if m.shouldPanic {
		panic("simulated WASM crash")
	}
	return &wasm.ResolveProcessResponse{}, nil
}
func (m *mockResolver) RegisterResolve(*wasm.RegisterResolveRequest) {}
func (m *mockResolver) ApplyFlags(*resolver.ApplyFlagsRequest) error { return nil }
func (m *mockResolver) FlushAllLogs() error                          { return nil }
func (m *mockResolver) FlushAssignLogs() error                       { return nil }
func (m *mockResolver) PrometheusSnapshot(_ uint32, _ bool) string   { return "" }
func (m *mockResolver) Close(context.Context) error                  { return nil }

// mockFactory returns a panicking mockResolver on the first call and
// healthy ones on subsequent calls. All instances share the same
// concrete type so atomic.Value.Store is happy when the factory is
// used correctly (i.e. the inner factory, not the wrapping one).
type mockFactory struct {
	count atomic.Int32
}

func (f *mockFactory) New() LocalResolver {
	n := f.count.Add(1)
	return &mockResolver{shouldPanic: n == 1}
}

func (f *mockFactory) Close(context.Context) error { return nil }

// TestRecoveringResolver_RecreatesAfterPanic verifies that after a panic,
// the RecoveringResolver recreates the inner resolver via the factory and
// subsequent calls succeed.
func TestRecoveringResolver_RecreatesAfterPanic(t *testing.T) {
	inner := &mockFactory{}
	factory := NewRecoveringResolverFactory(inner)
	rr := factory.New()

	// First call: the mockResolver (shouldPanic=true) panics; withRecover
	// recreates synchronously before returning.
	_, err := rr.ResolveProcess(&wasm.ResolveProcessRequest{})
	if err == nil {
		t.Fatal("expected error from panicking resolver")
	}

	if inner.count.Load() < 2 {
		t.Fatal("factory was never called to recreate the resolver")
	}

	// Second call: should succeed because the recovered resolver is a
	// fresh mockResolver(shouldPanic=false) from the inner factory.
	_, err = rr.ResolveProcess(&wasm.ResolveProcessRequest{})
	if err != nil {
		t.Fatalf("expected successful resolve after recovery, got: %v", err)
	}
}

// TestRecoveringResolver_ConcurrentResolveDuringRecovery verifies that a
// concurrent resolve cannot observe a half-swapped or closed WASM instance.
func TestRecoveringResolver_ConcurrentResolveDuringRecovery(t *testing.T) {
	inner := &mockFactory{}
	factory := NewRecoveringResolverFactory(inner)
	rr := factory.New()

	var wg sync.WaitGroup
	wg.Add(2)
	for range 2 {
		go func() {
			defer wg.Done()
			_, _ = rr.ResolveProcess(&wasm.ResolveProcessRequest{})
		}()
	}
	wg.Wait()

	if inner.count.Load() < 2 {
		t.Fatal("expected resolver recreation during concurrent resolves")
	}

	_, err := rr.ResolveProcess(&wasm.ResolveProcessRequest{})
	if err != nil {
		t.Fatalf("expected successful resolve after concurrent recovery, got: %v", err)
	}
}
