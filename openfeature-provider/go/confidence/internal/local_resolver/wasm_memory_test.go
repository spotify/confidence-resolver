package local_resolver

import (
	"context"
	"testing"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
	tu "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
)

// TestWasmMemoryStableOnRepeatedResolveCalls verifies that WASM linear memory
// does not grow unboundedly when resolving flags repeatedly. Each resolve_flags
// call triggers a current_time() host call from the guest. If the host function
// does not free the guest's request allocation, memory leaks accumulate and
// eventually force WASM memory.grow.
func TestWasmMemoryStableOnRepeatedResolveCalls(t *testing.T) {
	factory := NewWasmResolverFactory(NoOpLogSink)
	defer factory.Close(context.Background())

	resolver := factory.New()
	defer resolver.Close(context.Background())

	wasmResolver := resolver.(*WasmResolver)

	testState := tu.LoadTestResolverState(t)
	testAcctID := tu.LoadTestAccountID(t)

	if err := wasmResolver.SetResolverState(&wasm.SetResolverStateRequest{
		State:     testState,
		AccountId: testAcctID,
	}); err != nil {
		t.Fatalf("Failed to set resolver state: %v", err)
	}

	request := tu.CreateResolveProcessRequest(tu.CreateTutorialFeatureRequest())

	// Warm up: let allocator settling and one-time growth complete.
	for i := 0; i < 50_000; i++ {
		if _, err := wasmResolver.ResolveProcess(request); err != nil {
			t.Fatalf("ResolveProcess failed during warmup: %v", err)
		}
		if i%1000 == 0 {
			wasmResolver.FlushAllLogs()
		}
	}

	memBefore := wasmResolver.instance.Memory().Size()

	// Run resolves. Each call triggers current_time() in the guest which
	// allocates a request in WASM memory. A leak here causes linear growth.
	iterations := 50_000
	for i := 0; i < iterations; i++ {
		if _, err := wasmResolver.ResolveProcess(request); err != nil {
			t.Fatalf("ResolveProcess failed at iteration %d: %v", i, err)
		}
		if i%1000 == 0 {
			wasmResolver.FlushAllLogs()
		}
	}

	memAfter := wasmResolver.instance.Memory().Size()

	if memAfter > memBefore {
		t.Errorf("WASM memory grew from %d to %d bytes (%d bytes / %d pages) after %d resolve calls — indicates a leak in host function memory management",
			memBefore, memAfter, memAfter-memBefore, (memAfter-memBefore)/65536, iterations)
	}
}
