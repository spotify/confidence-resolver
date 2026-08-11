package local_resolver

import (
	"context"
	"fmt"
	"testing"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
	tu "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
)

func setupBenchResolver(b *testing.B, useInterpreter bool) (LocalResolver, *wasm.ResolveProcessRequest) {
	b.Helper()
	factory := NewWasmResolverFactory(NoOpLogSink, useInterpreter)
	b.Cleanup(func() { _ = factory.Close(context.Background()) })

	resolver := factory.New()
	b.Cleanup(func() { _ = resolver.Close(context.Background()) })

	// LoadTestResolverState expects *testing.T; benchmarks share the same skip helper.
	t := &testing.T{}
	state := tu.LoadTestResolverState(t)
	accountID := tu.LoadTestAccountID(t)
	if t.Skipped() {
		b.Skip("benchmark requires data/resolver_state_current.pb at repo root")
	}

	if err := resolver.SetResolverState(&wasm.SetResolverStateRequest{
		State:     state,
		AccountId: accountID,
	}); err != nil {
		b.Fatalf("SetResolverState: %v", err)
	}
	return resolver, tu.CreateResolveProcessRequest(tu.CreateTutorialFeatureRequest())
}

func benchmarkResolveProcess(b *testing.B, useInterpreter bool) {
	resolver, request := setupBenchResolver(b, useInterpreter)

	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		if _, err := resolver.ResolveProcess(request); err != nil {
			b.Fatalf("ResolveProcess: %v", err)
		}
	}
}

func BenchmarkResolveProcess_JIT(b *testing.B) {
	benchmarkResolveProcess(b, false)
}

func BenchmarkResolveProcess_Interpreter(b *testing.B) {
	benchmarkResolveProcess(b, true)
}

// TestReportInterpreterOverhead prints a side-by-side comparison when run with:
//
//	go test ./confidence/internal/local_resolver/ -run TestReportInterpreterOverhead -v
func TestReportInterpreterOverhead(t *testing.T) {
	jit := testing.Benchmark(BenchmarkResolveProcess_JIT)
	interp := testing.Benchmark(BenchmarkResolveProcess_Interpreter)
	if jit.N == 0 || interp.N == 0 {
		t.Skip("benchmark did not run")
	}

	jitNs := float64(jit.NsPerOp())
	interpNs := float64(interp.NsPerOp())
	ratio := interpNs / jitNs

	t.Logf("ResolveProcess JIT:          %12.0f ns/op  %6.0f ops/s  %d allocs/op",
		jitNs, 1e9/jitNs, jit.AllocsPerOp())
	t.Logf("ResolveProcess Interpreter:  %12.0f ns/op  %6.0f ops/s  %d allocs/op",
		interpNs, 1e9/interpNs, interp.AllocsPerOp())
	t.Logf("Interpreter/JIT ratio: %.2fx slower", ratio)
	t.Log(fmt.Sprintf("Throughput delta: JIT %.0f ops/s vs interpreter %.0f ops/s", 1e9/jitNs, 1e9/interpNs))
}
