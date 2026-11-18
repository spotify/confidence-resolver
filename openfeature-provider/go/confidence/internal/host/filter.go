package host

import (
	"context"
	_ "embed"
	"fmt"

	"github.com/tetratelabs/wazero"
	"github.com/tetratelabs/wazero/api"
	"github.com/tetratelabs/wazero/experimental"
)

// dedupeFilterWasm is state-to-wasm's threaded dedupe filter (resolver/src/main/resources). It
// exports seen(i64) -> i32 and imports a shared env.memory.
//
//go:embed assets/dedupe-filter-threaded.wasm
var dedupeFilterWasm []byte

// Filter answers one of a module's dedup imports (apply_filter.seen, schema_filter.seen). Each
// resolver instance gets its own filter instance in its generation's runtime, linked wasm to wasm.
// A DedupeFilter's memory outlives generations: pass the same one to the next Resolver to keep
// dedup history across a module swap.
type Filter interface {
	// module returns the filter's wasm and the module that satisfies its env import (nil if none).
	module() (wasm []byte, env api.Module)
}

type fixedFilter int32

func (f fixedFilter) module() ([]byte, api.Module) { return constSeenModule(int32(f)), nil }

var (
	// NeverSeen reports every key as new: no dedup.
	NeverSeen Filter = fixedFilter(0)
	// AlwaysSeen reports every key as seen, suppressing everything the filter gates.
	AlwaysSeen Filter = fixedFilter(1)
)

// constSeenModule exports seen(i64) -> i32 returning v (0 or 1).
func constSeenModule(v int32) []byte {
	b := []byte{0, 'a', 's', 'm', 1, 0, 0, 0}
	b = section(b, 1, []byte{1, 0x60, 1, 0x7e, 1, 0x7f}) // (i64) -> i32
	b = section(b, 3, []byte{1, 0})
	exp := append([]byte{1, 4}, "seen"...)
	b = section(b, 7, append(exp, 0x00, 0))
	return section(b, 10, []byte{1, 4, 0, 0x41, byte(v), 0x0b}) // i32.const v
}

// DedupeFilter is a fixed-size filter memory in a runtime of its own, which outlives the
// generations whose filter instances import it.
type DedupeFilter struct {
	rt  wazero.Runtime
	mem api.Module
}

// NewDedupeFilter creates a filter memory of pages 64 KiB pages. interpreter must match the
// resolvers that use it.
func NewDedupeFilter(ctx context.Context, pages uint32, interpreter bool) (*DedupeFilter, error) {
	rt := NewRuntime(ctx, interpreter).r
	mem, err := rt.InstantiateWithConfig(ctx, sharedMemoryModule(pages, pages), wazero.NewModuleConfig().WithName("env"))
	if err != nil {
		_ = rt.Close(ctx)
		return nil, fmt.Errorf("filter memory: %w", err)
	}
	return &DedupeFilter{rt: rt, mem: mem}, nil
}

func (f *DedupeFilter) module() ([]byte, api.Module) { return dedupeFilterWasm, f.mem }

// Close releases the filter memory. Every Resolver using it must be closed first.
func (f *DedupeFilter) Close(ctx context.Context) error { return f.rt.Close(ctx) }

// linkedFilter is a Filter compiled into one generation's runtime.
type linkedFilter struct {
	compiled wazero.CompiledModule
	env      api.Module
}

func linkFilter(ctx context.Context, rt wazero.Runtime, f Filter) (linkedFilter, error) {
	wasm, env := f.module()
	compiled, err := rt.CompileModule(ctx, wasm)
	return linkedFilter{compiled, env}, err
}

func (f linkedFilter) instantiate(ctx context.Context, rt wazero.Runtime) (api.Module, error) {
	if f.env != nil {
		ctx = importFrom(ctx, map[string]api.Module{"env": f.env})
	}
	return rt.InstantiateModule(ctx, f.compiled, wazero.NewModuleConfig().WithName(""))
}

// importFrom resolves the named imports to the given wasm module instances, which may live in
// another runtime; other imports resolve by name in the instantiating runtime as usual.
func importFrom(ctx context.Context, mods map[string]api.Module) context.Context {
	return experimental.WithImportResolver(ctx, func(name string) api.Module { return mods[name] })
}
