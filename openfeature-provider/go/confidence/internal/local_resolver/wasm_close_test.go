package local_resolver

import (
	"context"
	"fmt"
	"strings"
	"testing"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
)

// A resolve on a closed instance must end in a recoverable panic, never in a
// nil fn.Call (in production that dies as a silent SIGSEGV, exit 139).
func TestResolveOnClosedInstancePanics(t *testing.T) {
	factory := NewWasmResolverFactory(NoOpLogSink)
	defer factory.Close(context.Background())

	resolver := factory.New()
	// Close the raw instance: WasmResolver.Close flushes logs and warms fnCache.
	if err := resolver.(*WasmResolver).instance.Close(context.Background()); err != nil {
		t.Fatalf("Close failed: %v", err)
	}

	defer func() {
		rec := recover()
		if rec == nil {
			t.Fatal("expected panic when resolving on a closed instance")
		}
		if s := fmt.Sprint(rec); strings.Contains(s, "nil pointer") {
			t.Fatalf("a nil function handle reached Call: %v", rec)
		}
	}()
	_, _ = resolver.ResolveProcess(&wasm.ResolveProcessRequest{})
	t.Fatal("unreachable: ResolveProcess should have panicked")
}
