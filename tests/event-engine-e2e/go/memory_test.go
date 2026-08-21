package main

import (
	"context"
	"encoding/binary"
	"fmt"
	"os"
	"runtime"
	"testing"
	"time"

	"github.com/tetratelabs/wazero"
	"github.com/tetratelabs/wazero/api"
)

func loadWasm(t *testing.T) (api.Module, wazero.Runtime) {
	t.Helper()
	ctx := context.Background()
	wasmBytes, err := os.ReadFile(wasmPath)
	if err != nil {
		t.Fatalf("Failed to read WASM: %v", err)
	}
	rt := wazero.NewRuntime(ctx)
	mod, err := rt.Instantiate(ctx, wasmBytes)
	if err != nil {
		t.Fatalf("Failed to instantiate WASM: %v", err)
	}
	return mod, rt
}

type testHost struct {
	mod   api.Module
	alloc api.Function
	free  api.Function
	track api.Function
	flush api.Function
}

func newTestHost(mod api.Module) *testHost {
	return &testHost{
		mod:   mod,
		alloc: mod.ExportedFunction("wasm_msg_alloc"),
		free:  mod.ExportedFunction("wasm_msg_free"),
		track: mod.ExportedFunction("wasm_msg_guest_track_event"),
		flush: mod.ExportedFunction("wasm_msg_guest_bounded_flush_events"),
	}
}

func (h *testHost) callRaw(ctx context.Context, fn api.Function, reqData []byte) ([]byte, error) {
	envelope := encodeRequest(reqData)
	results, err := h.alloc.Call(ctx, uint64(len(envelope)))
	if err != nil {
		return nil, fmt.Errorf("alloc: %w", err)
	}
	ptr := uint32(results[0])
	h.mod.Memory().Write(ptr, envelope)

	resResults, err := fn.Call(ctx, uint64(ptr))
	if err != nil {
		return nil, fmt.Errorf("call: %w", err)
	}
	resPtr := uint32(resResults[0])
	if resPtr == 0 {
		return nil, nil
	}

	sizeBytes, ok := h.mod.Memory().Read(resPtr-4, 4)
	if !ok {
		return nil, fmt.Errorf("read size failed")
	}
	totalSize := binary.LittleEndian.Uint32(sizeBytes)
	dataLen := totalSize - 4

	resData, ok := h.mod.Memory().Read(resPtr, dataLen)
	if !ok {
		return nil, fmt.Errorf("read data failed")
	}
	result := make([]byte, dataLen)
	copy(result, resData)

	_, _ = h.free.Call(ctx, uint64(resPtr))
	return decodeResponse(result)
}

func trackAndFlushCycle(t *testing.T, host *testHost, ctx context.Context, n int) {
	t.Helper()
	for i := 0; i < n; i++ {
		eventData := encodeTrackEventRequest(eventName, time.Now())
		if _, err := host.callRaw(ctx, host.track, eventData); err != nil {
			t.Fatalf("track failed at %d: %v", i, err)
		}
	}
	if _, err := host.callRaw(ctx, host.flush, []byte{}); err != nil {
		t.Fatalf("flush failed: %v", err)
	}
}

// TestWasmMemoryStable verifies that WASM linear memory does not grow
// across identical track+flush cycles. Warm up with the same load so
// the allocator reaches steady-state before measuring.
func TestWasmMemoryStable(t *testing.T) {
	mod, rt := loadWasm(t)
	ctx := context.Background()
	defer rt.Close(ctx)
	host := newTestHost(mod)

	batchSize := 10_000

	// Warm up with 10 full-size cycles so allocator reaches steady-state
	for i := 0; i < 10; i++ {
		trackAndFlushCycle(t, host, ctx, batchSize)
	}
	memBefore := mod.Memory().Size()

	// Run 20 more identical cycles
	for i := 0; i < 20; i++ {
		trackAndFlushCycle(t, host, ctx, batchSize)
	}
	memAfter := mod.Memory().Size()

	growthPages := (memAfter - memBefore) / 65536
	t.Logf("WASM memory: before=%d after=%d (delta=%d bytes, %d pages)",
		memBefore, memAfter, memAfter-memBefore, growthPages)

	// Allow up to 16 pages (~1MB) of allocator fragmentation for large batches.
	// WASM linear memory can only grow, never shrink. dlmalloc inside WASM
	// may fragment with varying-size protobuf encoding buffers.
	if growthPages > 16 {
		t.Errorf("WASM memory grew by %d pages — exceeds fragmentation tolerance, possible leak",
			growthPages)
	}
}

// TestWasmMemoryStableSmallBatches tests with frequent small batches
// to stress the alloc/free path more than the queue itself.
func TestWasmMemoryStableSmallBatches(t *testing.T) {
	mod, rt := loadWasm(t)
	ctx := context.Background()
	defer rt.Close(ctx)
	host := newTestHost(mod)

	// Warm up
	for i := 0; i < 1000; i++ {
		trackAndFlushCycle(t, host, ctx, 10)
	}
	memBefore := mod.Memory().Size()

	// 5000 tiny track+flush cycles — stresses alloc/free churn
	for i := 0; i < 5000; i++ {
		trackAndFlushCycle(t, host, ctx, 10)
	}
	memAfter := mod.Memory().Size()

	t.Logf("WASM memory: before=%d after=%d (delta=%d bytes, %d pages) over 5000 small flush cycles",
		memBefore, memAfter, memAfter-memBefore, (memAfter-memBefore)/65536)

	if memAfter > memBefore {
		t.Errorf("WASM memory grew by %d bytes (%d pages) — alloc/free churn leak",
			memAfter-memBefore, (memAfter-memBefore)/65536)
	}
}

// TestGoHeapStable checks that the Go-side heap doesn't leak
// (e.g. from not copying WASM memory before freeing).
func TestGoHeapStable(t *testing.T) {
	mod, rt := loadWasm(t)
	ctx := context.Background()
	defer rt.Close(ctx)
	host := newTestHost(mod)

	// Warm up
	trackAndFlushCycle(t, host, ctx, 1000)
	runtime.GC()

	var memBefore runtime.MemStats
	runtime.ReadMemStats(&memBefore)

	for cycle := 0; cycle < 20; cycle++ {
		trackAndFlushCycle(t, host, ctx, 1000)
	}
	runtime.GC()

	var memAfter runtime.MemStats
	runtime.ReadMemStats(&memAfter)

	heapGrowthMB := float64(int64(memAfter.HeapAlloc)-int64(memBefore.HeapAlloc)) / 1024 / 1024
	t.Logf("Go heap: before=%dKB after=%dKB growth=%.2fMB",
		memBefore.HeapAlloc/1024, memAfter.HeapAlloc/1024, heapGrowthMB)

	if heapGrowthMB > 5 {
		t.Errorf("Go heap grew %.2fMB — possible Go-side leak", heapGrowthMB)
	}
}

// BenchmarkTrackEvent measures per-event tracking overhead.
func BenchmarkTrackEvent(b *testing.B) {
	ctx := context.Background()
	wasmBytes, _ := os.ReadFile(wasmPath)
	rt := wazero.NewRuntime(ctx)
	defer rt.Close(ctx)
	mod, _ := rt.Instantiate(ctx, wasmBytes)
	host := newTestHost(mod)

	eventData := encodeTrackEventRequest(eventName, time.Now())

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		host.callRaw(ctx, host.track, eventData)
	}
}

// BenchmarkFlush measures flush overhead for various batch sizes.
func BenchmarkFlush(b *testing.B) {
	for _, size := range []int{100, 1000, 10000} {
		b.Run(fmt.Sprintf("size=%d", size), func(b *testing.B) {
			ctx := context.Background()
			wasmBytes, _ := os.ReadFile(wasmPath)
			rt := wazero.NewRuntime(ctx)
			defer rt.Close(ctx)
			mod, _ := rt.Instantiate(ctx, wasmBytes)
			host := newTestHost(mod)

			b.ResetTimer()
			for i := 0; i < b.N; i++ {
				for j := 0; j < size; j++ {
					eventData := encodeTrackEventRequest(eventName, time.Now())
					host.callRaw(ctx, host.track, eventData)
				}
				host.callRaw(ctx, host.flush, []byte{})
			}
		})
	}
}
