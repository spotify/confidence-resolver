package local_resolver

import (
	"context"
	"fmt"
	"strings"
	"testing"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
)

// A resolve on a closed instance must end in a recoverable panic that
// RecoveringResolver turns into an error. It must never reach fn.Call through
// a nil function handle: in production that nil call dies as an unattributable
// SIGSEGV with no traceback (exit 139).
func TestResolveOnClosedInstancePanics(t *testing.T) {
	factory := NewWasmResolverFactory(NoOpLogSink)
	defer factory.Close(context.Background())

	resolver := factory.New()
	// Close the instance directly, bypassing WasmResolver.Close: its log flush
	// would populate fnCache, and this path must exercise the post-close
	// ExportedFunction lookup (as in the recovery race).
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
