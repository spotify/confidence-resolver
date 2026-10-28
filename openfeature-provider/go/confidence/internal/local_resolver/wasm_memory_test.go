package local_resolver

import (
	"context"
	"testing"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
	tu "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
	"google.golang.org/protobuf/encoding/protowire"
)

// TestWasmMemoryStableOnRepeatedResolveCalls verifies that WASM linear memory
// does not grow unboundedly when resolving flags repeatedly. Each resolve_flags
// call triggers a current_time() host call from the guest. If the host function
// does not free the guest's request allocation, memory leaks accumulate and
// eventually force WASM memory.grow.
func TestWasmMemoryStableOnRepeatedResolveCalls(t *testing.T) {
	factory := NewWasmResolverFactory(NoOpLogSink, false)
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

// TestWasmMemoryBoundedAcrossStateUpdates verifies that reloading a large
// resolver state does not grow WASM linear memory per update. Before the guest
// decoded the request as a view of the host buffer, each update held three
// copies of the encoding next to the previous state, and the instance settled
// at over twice the size of one.
func TestWasmMemoryBoundedAcrossStateUpdates(t *testing.T) {
	factory := NewWasmResolverFactory(NoOpLogSink, false)
	defer factory.Close(context.Background())

	resolver := factory.New()
	defer resolver.Close(context.Background())
	wasmResolver := resolver.(*WasmResolver)

	// The checked-in state is small; an unknown field that prost skips pads
	// the encoding to a size where page growth is visible.
	const padding = 32 << 20
	state := tu.LoadTestResolverState(t)
	state = append(state, 0xc2, 0xd3, 0x07) // field 15000, wire type 2
	state = protowire.AppendVarint(state, padding)
	state = append(state, make([]byte, padding)...)
	request := &wasm.SetResolverStateRequest{State: state, AccountId: tu.LoadTestAccountID(t)}
	resolve := tu.CreateResolveProcessRequest(tu.CreateTutorialFeatureRequest())

	var sizes []uint32
	for i := 0; i < 5; i++ {
		if err := wasmResolver.SetResolverState(request); err != nil {
			t.Fatalf("SetResolverState %d: %v", i+1, err)
		}
		for j := 0; j < 1000; j++ {
			if _, err := wasmResolver.ResolveProcess(resolve); err != nil {
				t.Fatalf("ResolveProcess: %v", err)
			}
		}
		if err := wasmResolver.FlushAllLogs(); err != nil {
			t.Fatalf("FlushAllLogs: %v", err)
		}
		sizes = append(sizes, wasmResolver.instance.Memory().Size())
	}
	t.Logf("WASM memory after each load: %v bytes", sizes)

	if last := sizes[len(sizes)-1]; last > sizes[1] {
		t.Errorf("WASM memory grew from %d to %d bytes between the second and fifth load", sizes[1], last)
	}
	if limit := uint32(2*len(state) + 16<<20); sizes[len(sizes)-1] > limit {
		t.Errorf("WASM memory %d bytes exceeds %d (twice the encoded state plus slack)", sizes[len(sizes)-1], limit)
	}
}
