// Package host runs compiled Confidence resolver modules (state-to-wasm, threaded variant) on wazero.
//
// The embedding contract is state-to-wasm's MEMORY.md; the reference host is its Java
// ConcurrentWasmResolver.
package host

import (
	"context"
	"errors"
	"fmt"

	"github.com/tetratelabs/wazero"
	"github.com/tetratelabs/wazero/api"
	"github.com/tetratelabs/wazero/experimental"
)

// ErrInvalidModule reports a module this host cannot run. Wrapped errors say why.
var ErrInvalidModule = errors.New("invalid resolver module")

const pageSize = 64 << 10

type signature struct {
	params, results []api.ValueType
}

func sig(params []api.ValueType, results ...api.ValueType) signature {
	return signature{params, results}
}

var (
	i32 = api.ValueTypeI32
	i64 = api.ValueTypeI64
	in  = func(t ...api.ValueType) []api.ValueType { return t }
)

// Imports a module may declare, besides host.memory. A module need not use all of them.
var knownImports = map[[2]string]signature{
	{"env", "write_logs"}:     sig(in(i64)),
	{"env", "current_time"}:   sig(in(), i64),
	{"env", "random_seed"}:    sig(in(), i64),
	{"apply_filter", "seen"}:  sig(in(i64), i32),
	{"schema_filter", "seen"}: sig(in(i64), i32),
}

var requiredExports = map[string]signature{
	"alloc":                   sig(in(i32), i32),
	"free":                    sig(in(i32)),
	"resolve_process_start":   sig(in(i64), i64),
	"resolve_process_resume":  sig(in(i32, i64), i64),
	"resolve_process_discard": sig(in(i32)),
	"apply_flags":             sig(in(i64), i32),
	"flush_logs":              sig(in()),
	"ack_logs":                sig(in(i64)),
	"destroy_instance":        sig(in()),
	"evaluate":                sig(in(i64, i64), i64),
}

// Optional exports are used when present. A present export with the wrong signature is rejected.
var optionalExports = map[string]signature{
	"allocated_bytes":                sig(in(), i32),
	"metrics_snapshot":               sig(in(i32), i64),
	"observe_client_resolve_latency": sig(in(i32, i64)),
	"unpack_bitsets":                 sig(in()),
}

// Runtime is a wazero runtime configured for threaded resolver modules.
type Runtime struct {
	r wazero.Runtime
}

// NewRuntime creates a runtime with the threads proposal enabled. interpreter selects wazero's
// interpreter instead of its compiler.
func NewRuntime(ctx context.Context, interpreter bool) *Runtime {
	cfg := wazero.NewRuntimeConfigCompiler()
	if interpreter {
		cfg = wazero.NewRuntimeConfigInterpreter()
	}
	cfg = cfg.WithCoreFeatures(api.CoreFeaturesV2 | experimental.CoreFeaturesThreads)
	return &Runtime{r: wazero.NewRuntimeWithConfig(ctx, cfg)}
}

// Close releases the runtime and everything compiled or instantiated in it.
func (rt *Runtime) Close(ctx context.Context) error {
	return rt.r.Close(ctx)
}

// Module is a validated, compiled resolver module. It holds no memory or instances.
type Module struct {
	compiled wazero.CompiledModule
	// Declared limits of the host.memory import, in pages.
	MinPages, MaxPages uint32
	// Optional exports the module provides.
	Optional map[string]bool
}

// Compile validates and compiles a module. Validation failures wrap ErrInvalidModule.
func (rt *Runtime) Compile(ctx context.Context, wasm []byte) (*Module, error) {
	compiled, err := rt.r.CompileModule(ctx, wasm)
	if err != nil {
		return nil, fmt.Errorf("%w: %v", ErrInvalidModule, err)
	}
	m, err := validate(compiled)
	if err == nil {
		var shared bool
		if shared, err = memoryImportShared(wasm); err == nil && !shared {
			err = errors.New("host.memory must be shared (threaded module variant)")
		}
	}
	if err != nil {
		_ = compiled.Close(ctx)
		return nil, fmt.Errorf("%w: %v", ErrInvalidModule, err)
	}
	return m, nil
}

// Close releases the compiled code.
func (m *Module) Close(ctx context.Context) error {
	return m.compiled.Close(ctx)
}

func validate(c wazero.CompiledModule) (*Module, error) {
	m := &Module{compiled: c, Optional: map[string]bool{}}

	mems := c.ImportedMemories()
	if len(mems) != 1 {
		return nil, fmt.Errorf("expected one imported memory, got %d", len(mems))
	}
	if mod, name, _ := mems[0].Import(); mod != "host" || name != "memory" {
		return nil, fmt.Errorf("memory must be imported as host.memory, got %s.%s", mod, name)
	}
	// The shared flag isn't exposed by wazero; Compile checks it with memoryImportShared.
	maxPages, ok := mems[0].Max()
	if !ok {
		return nil, errors.New("host.memory must declare a maximum")
	}
	m.MinPages, m.MaxPages = mems[0].Min(), maxPages

	for _, f := range c.ImportedFunctions() {
		mod, name, _ := f.Import()
		want, ok := knownImports[[2]string{mod, name}]
		if !ok {
			return nil, fmt.Errorf("unknown import %s.%s", mod, name)
		}
		if err := checkSig(f, want); err != nil {
			return nil, fmt.Errorf("import %s.%s: %v", mod, name, err)
		}
	}

	exports := c.ExportedFunctions()
	for name, want := range requiredExports {
		f, ok := exports[name]
		if !ok {
			return nil, fmt.Errorf("missing export %s", name)
		}
		if err := checkSig(f, want); err != nil {
			return nil, fmt.Errorf("export %s: %v", name, err)
		}
	}
	for name, want := range optionalExports {
		f, ok := exports[name]
		if !ok {
			continue
		}
		if err := checkSig(f, want); err != nil {
			return nil, fmt.Errorf("export %s: %v", name, err)
		}
		m.Optional[name] = true
	}
	return m, nil
}

func checkSig(f api.FunctionDefinition, want signature) error {
	if !equal(f.ParamTypes(), want.params) || !equal(f.ResultTypes(), want.results) {
		return fmt.Errorf("signature %v -> %v, want %v -> %v",
			names(f.ParamTypes()), names(f.ResultTypes()), names(want.params), names(want.results))
	}
	return nil
}

func equal(a, b []api.ValueType) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

func names(ts []api.ValueType) []string {
	out := make([]string, len(ts))
	for i, t := range ts {
		out[i] = api.ValueTypeName(t)
	}
	return out
}
