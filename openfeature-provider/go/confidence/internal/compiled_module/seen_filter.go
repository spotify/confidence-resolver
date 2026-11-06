package compiled_module

import (
	"context"
	_ "embed"
	"fmt"
	"log/slog"
	"sync"

	"github.com/tetratelabs/wazero"
	"github.com/tetratelabs/wazero/api"
	"github.com/tetratelabs/wazero/experimental"
)

//go:embed assets/dedupe-filter-threaded.wasm
var seenFilterWasm []byte

// SeenFilter owns a persistent WASM de-duplication filter. It retains its
// memory across client-module replacements. Calls serialize because a wazero
// function cannot be entered concurrently.
type SeenFilter struct {
	mu       sync.Mutex
	disabled bool
	pages    uint32
	runtime  wazero.Runtime
	seen     api.Function
	closed   bool
}

func NewSeenFilter(disabled bool, pages uint32) *SeenFilter {
	return &SeenFilter{disabled: disabled, pages: pages}
}

// Init allocates the filter once, using the provider's selected WASM engine.
func (f *SeenFilter) Init(useInterpreter bool) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.closed {
		return fmt.Errorf("seen filter is closed")
	}
	if f.disabled || f.runtime != nil {
		return nil
	}
	if f.pages == 0 {
		return fmt.Errorf("seen filter needs at least one memory page")
	}
	ctx := context.Background()
	cfg := wazero.NewRuntimeConfigCompiler()
	if useInterpreter {
		cfg = wazero.NewRuntimeConfigInterpreter()
	}
	cfg = cfg.WithCoreFeatures(api.CoreFeaturesV2 | experimental.CoreFeaturesThreads)
	r := wazero.NewRuntimeWithConfig(ctx, cfg)
	// The filter imports env.memory, independently of the client's host.memory.
	if _, err := r.InstantiateWithConfig(ctx, sharedMemoryHelperWasm(f.pages, f.pages, true), wazero.NewModuleConfig().WithName("env")); err != nil {
		r.Close(ctx)
		return fmt.Errorf("seen filter memory: %w", err)
	}
	mod, err := r.Instantiate(ctx, seenFilterWasm)
	if err != nil {
		r.Close(ctx)
		return fmt.Errorf("seen filter module: %w", err)
	}
	seen := mod.ExportedFunction("seen")
	if seen == nil {
		r.Close(ctx)
		return fmt.Errorf("seen filter module missing seen export")
	}
	f.runtime = r
	f.seen = seen
	return nil
}

// Seen records new fingerprints and reports duplicates. A filter failure
// fails open so an exposure is still reported.
func (f *SeenFilter) Seen(fingerprint int64) bool {
	if f == nil {
		return false
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.disabled || f.closed || f.seen == nil {
		return false
	}
	result, err := f.seen.Call(context.Background(), uint64(fingerprint))
	if err != nil {
		slog.Error("seen filter failed", "error", err)
		return false
	}
	return api.DecodeU32(result[0]) != 0
}

func (f *SeenFilter) Close() {
	if f == nil {
		return
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.runtime != nil {
		_ = f.runtime.Close(context.Background())
	}
	f.runtime = nil
	f.seen = nil
	f.closed = true
}
