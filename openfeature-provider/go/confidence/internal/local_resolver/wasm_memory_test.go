package local_resolver

import (
	"context"
	"fmt"
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

// TestWasmMemoryStableOnRepeatedSetResolverState verifies that WASM linear
// memory does not grow unboundedly when SetResolverState is called repeatedly
// on the same instance — simulating the production state polling loop (every 10s).
// WASM linear memory can grow but never shrink, so if old state is not freed
// before new state is allocated, each update leaks the previous state's worth
// of memory.
func TestWasmMemoryStableOnRepeatedSetResolverState(t *testing.T) {
	factory := NewWasmResolverFactory(NoOpLogSink)
	defer factory.Close(context.Background())

	var wasmResolvers []*WasmResolver
	supplier := func() LocalResolver {
		lr := factory.New()
		wasmResolvers = append(wasmResolvers, lr.(*WasmResolver))
		return lr
	}

	pooled := NewPooledResolver(DefaultPoolSize, supplier)
	defer pooled.Close(context.Background())

	testState := tu.LoadTestResolverState(t)
	testAcctID := tu.LoadTestAccountID(t)

	request := &wasm.SetResolverStateRequest{
		State:     testState,
		AccountId: testAcctID,
	}

	totalMemory := func() uint32 {
		var total uint32
		for _, wr := range wasmResolvers {
			total += wr.instance.Memory().Size()
		}
		return total
	}

	// Warm up: let one-time allocator growth and initial state settle.
	warmupIterations := 10
	for i := 0; i < warmupIterations; i++ {
		if err := pooled.SetResolverState(request); err != nil {
			t.Fatalf("SetResolverState failed during warmup at iteration %d: %v", i, err)
		}
	}

	memBefore := totalMemory()

	// Simulate the production polling loop: repeated SetResolverState on the
	// same long-lived pool. In production this happens every 10s with ~240KB
	// state payloads applied to all slots via maintenance(). A leak here
	// causes monotonic memory growth.
	iterations := 200
	for i := 0; i < iterations; i++ {
		if err := pooled.SetResolverState(request); err != nil {
			t.Fatalf("SetResolverState failed at iteration %d: %v", i, err)
		}
	}

	memAfter := totalMemory()

	if memAfter > memBefore {
		t.Errorf("WASM pool memory grew from %d to %d bytes (+%d bytes / +%d pages) after %d SetResolverState calls — "+
			"indicates old state is not freed before new state is allocated",
			memBefore, memAfter, memAfter-memBefore, (memAfter-memBefore)/65536, iterations)
	} else {
		fmt.Printf("WASM pool memory stable at %d bytes after %d SetResolverState calls\n", memAfter, iterations)
	}
}

// TestWasmMemoryStableOnRepeatedSetResolverStateWithResolves simulates the
// real production pattern using a PooledResolver: state updates (applied to
// all slots via maintenance()) interleaved with round-robined resolve traffic.
// Between each SetResolverState, multiple resolves are performed (as would
// happen during the 10s poll interval). This can cause heap fragmentation
// in the WASM allocator that pure state-only tests would miss.
func TestWasmMemoryStableOnRepeatedSetResolverStateWithResolves(t *testing.T) {
	factory := NewWasmResolverFactory(NoOpLogSink)
	defer factory.Close(context.Background())

	var wasmResolvers []*WasmResolver
	supplier := func() LocalResolver {
		lr := factory.New()
		wasmResolvers = append(wasmResolvers, lr.(*WasmResolver))
		return lr
	}

	pooled := NewPooledResolver(DefaultPoolSize, supplier)
	defer pooled.Close(context.Background())

	testState := tu.LoadTestResolverState(t)
	testAcctID := tu.LoadTestAccountID(t)

	stateRequest := &wasm.SetResolverStateRequest{
		State:     testState,
		AccountId: testAcctID,
	}
	resolveRequest := tu.CreateResolveProcessRequest(tu.CreateTutorialFeatureRequest())

	totalMemory := func() uint32 {
		var total uint32
		for _, wr := range wasmResolvers {
			total += wr.instance.Memory().Size()
		}
		return total
	}

	// Warm up: initial state load + resolve traffic to settle allocator.
	if err := pooled.SetResolverState(stateRequest); err != nil {
		t.Fatalf("initial SetResolverState failed: %v", err)
	}
	for i := 0; i < 1000; i++ {
		if _, err := pooled.ResolveProcess(resolveRequest); err != nil {
			t.Fatalf("warmup ResolveProcess failed: %v", err)
		}
	}

	memBefore := totalMemory()

	// Simulate production: each "tick" does 1 state update across all pool
	// slots + N resolves round-robined across them, modeling the 10s poll
	// with concurrent resolve traffic.
	stateUpdates := 200
	resolvesPerTick := 100
	for tick := 0; tick < stateUpdates; tick++ {
		if err := pooled.SetResolverState(stateRequest); err != nil {
			t.Fatalf("SetResolverState failed at tick %d: %v", tick, err)
		}
		for j := 0; j < resolvesPerTick; j++ {
			if _, err := pooled.ResolveProcess(resolveRequest); err != nil {
				t.Fatalf("ResolveProcess failed at tick %d resolve %d: %v", tick, j, err)
			}
		}
		if tick%50 == 0 {
			pooled.FlushAllLogs()
		}
	}

	memAfter := totalMemory()

	fmt.Printf("Pool memory (%d slots): before=%d after=%d delta=%d bytes (%d pages) over %d state updates with %d resolves each\n",
		len(wasmResolvers), memBefore, memAfter, memAfter-memBefore, (memAfter-memBefore)/65536, stateUpdates, resolvesPerTick)

	if memAfter > memBefore {
		t.Errorf("WASM pool memory grew from %d to %d bytes (+%d bytes / +%d pages) after %d state updates interleaved with resolves — "+
			"indicates heap fragmentation or leak during state+resolve cycles",
			memBefore, memAfter, memAfter-memBefore, (memAfter-memBefore)/65536, stateUpdates)
	}
}
