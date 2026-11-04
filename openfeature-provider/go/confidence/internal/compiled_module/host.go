package compiled_module

import (
	"context"
	"crypto/rand"
	"encoding/binary"
	"fmt"
	"log/slog"
	"runtime"
	"sync"
	"time"

	"github.com/tetratelabs/wazero"
	"github.com/tetratelabs/wazero/api"
	"github.com/tetratelabs/wazero/experimental"
	"google.golang.org/protobuf/proto"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/flagswasm"
)

// LogSink receives the concatenated bytes of one WriteFlagLogsRequest
// produced by the module. It is called from inside a WASM call, so it must
// not block, must not re-enter the module, and must not panic; failures are
// the sink's business and must never surface into a resolve. The token to
// attach at delivery time is captured by the caller of NewHost (it is not
// part of the module).
type LogSink func(body []byte)

// Host is one live compiled client module: a wazero runtime, the module, the
// one shared memory the module's flag data lives in, and a pool of thin
// instances serving concurrent calls.
//
// The design follows the module contract's threading model: the flag data is
// initialized into the shared memory once (by the first instantiation's
// start function) and instances are runtime-equal execution contexts over
// that memory. One instance serves one in-flight call at a time — instances
// are never entered concurrently — and the pool lets evaluations from
// different host goroutines run in parallel, each on its own instance.
//
// A resolve flow (start → evaluate → resume/discard → free) is pinned to the
// instance that started it, matching the contract's rule that an in-flight
// call never migrates between host threads. Module-wide entry points
// (apply_flags, flush_logs, metrics, gauges) may use any instance.
// defaultInstanceCap bounds the default pool size; see HostConfig.
const defaultInstanceCap = 4

type Host struct {
	runtime  wazero.Runtime
	compiled wazero.CompiledModule
	memory   api.Memory
	mod      *Module
	closers  []api.Closer

	// allocator owns the reserved shared-memory region; it is released
	// after the runtime, never by wazero's per-instance cleanup.
	allocator *lazyMemoryAllocator

	// mu guards the lifecycle state and instance accounting. It is NOT an
	// entry lock: instance calls run outside it.
	mu     sync.Mutex
	closed bool
	free   chan *instance
	live   int
	cap    int

	logSink LogSink

	hasMetricsSnapshot bool
	hasAllocatedBytes  bool
	hasObserveLatency  bool
}

// instance is one instantiation of the client module over the shared memory,
// with its resolved export handles. It serves one call at a time; the pool
// in Host hands it to one goroutine at a time.
type instance struct {
	mod   api.Module
	alloc api.Function
	frees api.Function

	resolveProcessStart   api.Function
	resolveProcessResume  api.Function
	resolveProcessDiscard api.Function
	applyFlags            api.Function
	flushLogs             api.Function
	evaluate              api.Function
	metricsSnapshot       api.Function
	allocatedBytes        api.Function
	observeLatency        api.Function
}

// HostConfig carries the host-provided callbacks.
type HostConfig struct {
	// LogSink receives log batches (see LogSink). Required.
	LogSink LogSink
	// Now returns wall-clock milliseconds since the epoch. Optional.
	Now func() int64
	// Seed returns a random value used once per instance. Optional.
	Seed func() int64
	// UseInterpreter runs the module on wazero's interpreter engine instead
	// of its compiler. The signal-handling trade-offs documented in the
	// provider README apply. Optional.
	UseInterpreter bool
	// InstanceCap bounds the instance pool: one instance per concurrent
	// call, up to this many. 0 defaults to min(GOMAXPROCS, 4) — the
	// measured p99 knee. Additional callers wait for an instance to be
	// returned. Optional.
	InstanceCap int
	// ApplySeen and SchemaSeen implement the exposure/schema de-duplication
	// imports: they return true when the fingerprint was already reported.
	// The filters are shared host-side across all instances.
	// Nil disables de-duplication for that namespace (always false).
	ApplySeen  func(fingerprint int64) bool
	SchemaSeen func(fingerprint int64) bool
	// Logger for callback failures. Optional.
	Logger *slog.Logger
}

func (c HostConfig) now() int64 {
	if c.Now != nil {
		return c.Now()
	}
	return time.Now().UnixMilli()
}

func (c HostConfig) seed() int64 {
	if c.Seed != nil {
		return c.Seed()
	}
	var b [8]byte
	if _, err := rand.Read(b[:]); err != nil {
		return time.Now().UnixNano()
	}
	return int64(binary.LittleEndian.Uint64(b[:]))
}

func (c HostConfig) logger() *slog.Logger {
	if c.Logger != nil {
		return c.Logger
	}
	return slog.Default()
}

// NewHost compiles and instantiates a validated module on wazero with the
// threads feature enabled: the module's atomic instructions decode, and the
// shared host.memory it imports is provided by a tiny helper module that
// defines and exports a shared memory with exactly the limits the client
// module declares. One instance is warmed eagerly (its start function
// initializes the flag data into the shared memory); the pool grows lazily
// to the configured cap as concurrent calls arrive. Close tears everything
// down.
func NewHost(mod *Module, cfg HostConfig) (*Host, error) {
	if cfg.LogSink == nil {
		return nil, errNilLogSink
	}

	ctx := context.Background()
	runtimeCfg := wazero.NewRuntimeConfigCompiler()
	if cfg.UseInterpreter {
		runtimeCfg = wazero.NewRuntimeConfigInterpreter()
	}
	runtimeCfg = runtimeCfg.WithCoreFeatures(api.CoreFeaturesV2 | experimental.CoreFeaturesThreads)
	r := wazero.NewRuntimeWithConfig(ctx, runtimeCfg)

	cap := cfg.InstanceCap
	if cap <= 0 {
		// Default cap: GOMAXPROCS bounded by 4. Measurements (10-vCPU
		// aarch64, 10 concurrent evaluators) show the best p99 at 2-4
		// instances, with p99 regressing at GOMAXPROCS-scale pools from
		// vCPU and GC contention, while throughput at 4 is already far
		// beyond any realistic flag-evaluation load.
		cap = runtime.GOMAXPROCS(0)
		if cap > defaultInstanceCap {
			cap = defaultInstanceCap
		}
	}

	h := &Host{
		runtime:            r,
		mod:                mod,
		logSink:            cfg.LogSink,
		cap:                cap,
		free:               make(chan *instance, cap),
		hasMetricsSnapshot: mod.HasExport("metrics_snapshot"),
		hasAllocatedBytes:  mod.HasExport("allocated_bytes"),
		hasObserveLatency:  mod.HasExport("observe_client_resolve_latency"),
	}

	// Host functions required by the module contract. Instantiating a
	// namespace the module does not import is harmless: wazero resolves
	// only the imports the module declares.
	env := r.NewHostModuleBuilder("env").
		NewFunctionBuilder().WithFunc(func() int64 { return cfg.now() }).Export("current_time").
		NewFunctionBuilder().WithFunc(func() int64 { return cfg.seed() }).Export("random_seed")
	if mod.HasImport("env", "write_logs") {
		env = env.NewFunctionBuilder().WithFunc(
			func(callCtx context.Context, caller api.Module, payload uint64) {
				h.deliverLogs(caller, payload, cfg.logger())
			},
		).Export("write_logs")
	}
	if envMod, err := env.Instantiate(ctx); err != nil {
		r.Close(ctx)
		return nil, fmt.Errorf("env host module: %w", err)
	} else {
		h.closers = append(h.closers, envMod)
	}
	if mod.HasImport("apply_filter", "seen") {
		seen := cfg.ApplySeen
		if fMod, err := r.NewHostModuleBuilder("apply_filter").
			NewFunctionBuilder().WithFunc(func(fingerprint int64) int32 {
			return boolToI32(seen != nil && seen(fingerprint))
		}).Export("seen").
			Instantiate(ctx); err != nil {
			r.Close(ctx)
			return nil, fmt.Errorf("apply_filter host module: %w", err)
		} else {
			h.closers = append(h.closers, fMod)
		}
	}
	if mod.HasImport("schema_filter", "seen") {
		seen := cfg.SchemaSeen
		if fMod, err := r.NewHostModuleBuilder("schema_filter").
			NewFunctionBuilder().WithFunc(func(fingerprint int64) int32 {
			return boolToI32(seen != nil && seen(fingerprint))
		}).Export("seen").
			Instantiate(ctx); err != nil {
			r.Close(ctx)
			return nil, fmt.Errorf("schema_filter host module: %w", err)
		} else {
			h.closers = append(h.closers, fMod)
		}
	}

	// host.memory: a helper module defining and exporting a shared memory
	// with the module's declared limits. All client instances import this
	// same memory; wazero allocates shared memories fixed at their maximum,
	// which is what threads semantics expect.
	limits := mod.info.HostMemoryLimits
	alloc := &lazyMemoryAllocator{}
	h.allocator = alloc
	helperCtx := experimental.WithMemoryAllocator(ctx, alloc)
	helper, err := r.InstantiateWithConfig(helperCtx, sharedMemoryHelperWasm(limits.Initial, limits.Maximum, limits.HasMax), wazero.NewModuleConfig().WithName("host"))
	if err != nil {
		r.Close(ctx)
		alloc.Release()
		return nil, fmt.Errorf("shared memory helper: %w", err)
	}
	h.closers = append(h.closers, helper)
	h.memory = helper.Memory()
	if h.memory == nil {
		r.Close(ctx)
		return nil, fmt.Errorf("shared memory helper exported no memory")
	}

	// Compile the client module once and instantiate it many times: every
	// instance shares this compiled code. Instantiating from bytes instead
	// would give each instance ownership of its code (wazero sets
	// closeWithModule), so closing one instance would release the code
	// under the others.
	compiled, err := r.CompileModule(ctx, mod.Bytes)
	if err != nil {
		r.Close(ctx)
		alloc.Release()
		return nil, err
	}
	h.compiled = compiled

	// Warm one instance; its start function initializes the flag data into
	// the shared memory.
	first, err := h.newInstance()
	if err != nil {
		_ = compiled.Close(ctx)
		r.Close(ctx)
		alloc.Release()
		return nil, err
	}
	h.free <- first
	h.live = 1
	return h, nil
}

// newInstance instantiates the compiled client module over the shared
// memory and resolves its export handles.
func (h *Host) newInstance() (*instance, error) {
	mod, err := h.runtime.InstantiateModule(context.Background(), h.compiled, wazero.NewModuleConfig())
	if err != nil {
		return nil, err
	}
	inst := &instance{
		mod:                   mod,
		alloc:                 mod.ExportedFunction("alloc"),
		frees:                 mod.ExportedFunction("free"),
		resolveProcessStart:   mod.ExportedFunction("resolve_process_start"),
		resolveProcessResume:  mod.ExportedFunction("resolve_process_resume"),
		resolveProcessDiscard: mod.ExportedFunction("resolve_process_discard"),
		applyFlags:            mod.ExportedFunction("apply_flags"),
		flushLogs:             mod.ExportedFunction("flush_logs"),
		evaluate:              mod.ExportedFunction("evaluate"),
		metricsSnapshot:       mod.ExportedFunction("metrics_snapshot"),
		allocatedBytes:        mod.ExportedFunction("allocated_bytes"),
		observeLatency:        mod.ExportedFunction("observe_client_resolve_latency"),
	}
	if inst.alloc == nil || inst.frees == nil ||
		inst.resolveProcessStart == nil || inst.resolveProcessResume == nil ||
		inst.resolveProcessDiscard == nil || inst.applyFlags == nil ||
		inst.flushLogs == nil || inst.evaluate == nil {
		_ = mod.Close(context.Background())
		return nil, errMissingExport
	}
	return inst, nil
}

// acquire returns an instance for exclusive use by one caller. The pool
// grows lazily up to its cap; beyond that, callers wait for an instance to
// be returned.
func (h *Host) acquire() (*instance, error) {
	h.mu.Lock()
	if h.closed {
		h.mu.Unlock()
		return nil, errHostClosed
	}
	if h.live < h.cap {
		// Room to grow: create outside the lock.
		h.live++
		h.mu.Unlock()
		inst, err := h.newInstance()
		if err != nil {
			h.mu.Lock()
			h.live--
			h.mu.Unlock()
			return nil, err
		}
		return inst, nil
	}
	h.mu.Unlock()
	// At the cap: wait for a returned instance. Close unblocks waiters by
	// closing the channel; the zero value reads as a nil instance.
	inst := <-h.free
	if inst == nil {
		return nil, errHostClosed
	}
	return inst, nil
}

// release returns an instance to the pool, destroying it if the host is
// already closed.
func (h *Host) release(inst *instance) {
	if inst == nil {
		return
	}
	h.mu.Lock()
	if h.closed {
		h.mu.Unlock()
		h.destroyInstance(inst)
		h.mu.Lock()
		h.live--
		h.mu.Unlock()
		return
	}
	h.free <- inst
	h.mu.Unlock()
}

// withInstance runs fn with any pool instance and returns it afterwards.
func (h *Host) withInstance(fn func(*instance) error) error {
	inst, err := h.acquire()
	if err != nil {
		return err
	}
	defer h.release(inst)
	return fn(inst)
}

// sharedMemoryHelperWasm emits a module equivalent to
// (module (memory (export "memory") <min> <max> shared)) — the artifact the
// client module's host.memory import resolves against.
func sharedMemoryHelperWasm(minPages, maxPages uint32, hasMax bool) []byte {
	var out []byte
	out = append(out, 0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00)

	// Memory section: one shared memory with the declared limits.
	var mem []byte
	mem = append(mem, 0x01) // one memory
	flags := byte(0x02)     // shared; | 0x01 when a maximum is declared
	if hasMax {
		flags |= 0x01
	}
	mem = append(mem, flags)
	mem = appendUleb(mem, uint64(minPages))
	if hasMax {
		mem = appendUleb(mem, uint64(maxPages))
	}
	out = append(out, 0x05)
	out = appendUleb(out, uint64(len(mem)))
	out = append(out, mem...)

	// Export section: "memory" (kind 2), index 0.
	var exp []byte
	exp = append(exp, 0x01)
	exp = append(exp, 0x06)
	exp = append(exp, 'm', 'e', 'm', 'o', 'r', 'y')
	exp = append(exp, 0x02, 0x00)
	out = append(out, 0x07)
	out = appendUleb(out, uint64(len(exp)))
	out = append(out, exp...)
	return out
}

func appendUleb(b []byte, v uint64) []byte {
	for {
		byte7 := byte(v & 0x7f)
		v >>= 7
		if v != 0 {
			byte7 |= 0x80
		}
		b = append(b, byte7)
		if v == 0 {
			return b
		}
	}
}

// deliverLogs implements the env.write_logs callback: copy the parts out of
// WASM memory while the callback runs, hand the body to the sink, and ack
// exactly once in a finally. It must never throw (a panic would trap the
// in-flight WASM call) and never re-enters the module other than through
// ack_logs, which the contract allows from any call.
func (h *Host) deliverLogs(caller api.Module, payload uint64, logger *slog.Logger) {
	acked := false
	defer func() {
		if !acked {
			h.ack(caller, payload)
		}
		if r := recover(); r != nil {
			logger.Error("write_logs callback panicked", "panic", r)
		}
	}()

	ptr, length := unpackSlice(payload)
	if length == 0 {
		return
	}
	data, ok := h.memory.Read(ptr, length)
	if !ok {
		logger.Error("write_logs payload out of memory bounds")
		return
	}

	envelope := &flagswasm.WriteLogsPayload{}
	if err := proto.Unmarshal(data, envelope); err != nil {
		logger.Error("cannot parse write_logs payload", "error", err)
		return
	}

	var body []byte
	for _, part := range envelope.GetParts() {
		pptr, plen := unpackSlice(part)
		pdata, ok := h.memory.Read(pptr, plen)
		if !ok {
			logger.Error("write_logs part out of memory bounds")
			return
		}
		body = append(body, pdata...)
	}
	if len(body) == 0 {
		return
	}
	h.logSink(body)
}

// ack acknowledges a delivered payload on the instance that produced it.
// The write_logs callback runs inside that instance's call, so this reenters
// through the calling module's own export (allowed by the contract:
// ack_logs may be called from any call) rather than taking a pool instance.
func (h *Host) ack(caller api.Module, payload uint64) {
	defer func() {
		// ack_logs must never throw; a failure here is logged and dropped.
		if r := recover(); r != nil {
			slog.Default().Error("ack_logs panicked", "panic", r)
		}
	}()
	ackFn := caller.ExportedFunction("ack_logs")
	if ackFn == nil {
		return
	}
	if _, err := ackFn.Call(context.Background(), payload); err != nil {
		slog.Default().Error("ack_logs failed", "error", err)
	}
}

// Close destroys every instance while the shared memory is still alive,
// then releases the runtime and its modules. It must not be called
// concurrently with in-flight calls (the provider drains first).
func (h *Host) Close() {
	h.mu.Lock()
	if h.closed {
		h.mu.Unlock()
		return
	}
	h.closed = true
	// Drain idle instances and destroy them; instances still out with
	// callers are destroyed by their release path.
	drained := 0
	for drained < len(h.free) {
		inst := <-h.free
		h.mu.Unlock()
		h.destroyInstance(inst)
		h.mu.Lock()
		drained++
		h.live--
	}
	// Unblock any waiters so acquire reports the closure.
	close(h.free)
	h.mu.Unlock()

	ctx := context.Background()
	for i := len(h.closers) - 1; i >= 0; i-- {
		_ = h.closers[i].Close(ctx)
	}
	h.closers = nil
	if h.compiled != nil {
		_ = h.compiled.Close(ctx)
		h.compiled = nil
	}
	_ = h.runtime.Close(ctx)
	// The shared memory region outlives the runtime; only now is it safe
	// to unmap.
	if h.allocator != nil {
		h.allocator.Release()
	}
}

// destroyInstance tears down one instance: the contract's destroy_instance
// export first, then the runtime handle.
func (h *Host) destroyInstance(inst *instance) {
	if inst == nil {
		return
	}
	if fn := inst.mod.ExportedFunction("destroy_instance"); fn != nil {
		if _, err := fn.Call(context.Background()); err != nil {
			slog.Default().Error("destroy_instance failed", "error", err)
		}
	}
	_ = inst.mod.Close(context.Background())
}

func boolToI32(b bool) int32 {
	if b {
		return 1
	}
	return 0
}
