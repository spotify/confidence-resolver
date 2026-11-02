package local_resolver

import (
	"context"
	"testing"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
	tu "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
)

func wasmMemorySize(t *testing.T, lr LocalResolver) uint32 {
	t.Helper()
	wr, ok := lr.(*WasmResolver)
	if !ok {
		t.Fatalf("slot holds %T, want *WasmResolver", lr)
	}
	return wr.instance.Memory().Size()
}

// TestPooledResolver_SetResolverStateReplacesLoadedInstances verifies that a
// state update does not grow WASM memory: the first load fills the existing
// instances in place, every later load swaps in a fresh instance.
func TestPooledResolver_SetResolverStateReplacesLoadedInstances(t *testing.T) {
	ctx := context.Background()
	factory := NewWasmResolverFactory(NoOpLogSink, false)
	defer factory.Close(ctx)

	pool := NewPooledResolver(1, factory.New)
	defer pool.Close(ctx)

	request := &wasm.SetResolverStateRequest{
		State:     tu.LoadTestResolverState(t),
		AccountId: tu.LoadTestAccountID(t),
	}

	initial := make([]LocalResolver, len(pool.slots))
	for i := range pool.slots {
		initial[i] = pool.slots[i].lr
	}
	if err := pool.SetResolverState(request); err != nil {
		t.Fatalf("first SetResolverState: %v", err)
	}
	for i := range pool.slots {
		if pool.slots[i].lr != initial[i] {
			t.Errorf("slot %d: first load replaced the instance", i)
		}
	}
	afterFirst := make([]uint32, len(pool.slots))
	for i := range pool.slots {
		afterFirst[i] = wasmMemorySize(t, pool.slots[i].lr)
	}

	for n := 0; n < 3; n++ {
		if err := pool.SetResolverState(request); err != nil {
			t.Fatalf("SetResolverState %d: %v", n+2, err)
		}
	}
	for i := range pool.slots {
		if pool.slots[i].lr == initial[i] {
			t.Errorf("slot %d: later load kept the initial instance", i)
		}
		if got := wasmMemorySize(t, pool.slots[i].lr); got != afterFirst[i] {
			t.Errorf("slot %d: WASM memory %d bytes after four loads, %d after one", i, got, afterFirst[i])
		}
	}

	resp, err := pool.ResolveProcess(tu.CreateResolveProcessRequest(tu.CreateTutorialFeatureRequest()))
	if err != nil {
		t.Fatalf("ResolveProcess after replacement: %v", err)
	}
	if resp.GetResult() == nil {
		t.Fatal("ResolveProcess returned no result")
	}
}

// TestPooledResolver_SetResolverStateKeepsOldInstanceOnFailure verifies that a
// state the fresh instance cannot load leaves the slot serving the old state.
func TestPooledResolver_SetResolverStateKeepsOldInstanceOnFailure(t *testing.T) {
	ctx := context.Background()
	factory := NewWasmResolverFactory(NoOpLogSink, false)
	defer factory.Close(ctx)

	pool := NewPooledResolver(1, factory.New)
	defer pool.Close(ctx)

	if err := pool.SetResolverState(&wasm.SetResolverStateRequest{
		State:     tu.LoadTestResolverState(t),
		AccountId: tu.LoadTestAccountID(t),
	}); err != nil {
		t.Fatalf("first SetResolverState: %v", err)
	}
	loaded := make([]LocalResolver, len(pool.slots))
	for i := range pool.slots {
		loaded[i] = pool.slots[i].lr
	}

	err := pool.SetResolverState(&wasm.SetResolverStateRequest{
		State:     []byte("not a resolver state"),
		AccountId: tu.LoadTestAccountID(t),
	})
	if err == nil {
		t.Fatal("SetResolverState with a corrupt state returned no error")
	}
	for i := range pool.slots {
		if pool.slots[i].lr != loaded[i] {
			t.Errorf("slot %d: failed load replaced the instance", i)
		}
	}
	if _, err := pool.ResolveProcess(tu.CreateResolveProcessRequest(tu.CreateTutorialFeatureRequest())); err != nil {
		t.Fatalf("ResolveProcess after failed load: %v", err)
	}
}
