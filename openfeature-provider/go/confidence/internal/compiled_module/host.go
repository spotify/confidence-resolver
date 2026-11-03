package compiled_module

import (
	"context"
	"crypto/rand"
	"encoding/binary"
	"fmt"
	"log/slog"
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
// shared memory the module's flag data lives in, and a single instance
// entered exclusively through a mutex.
//
// The single-instance/single-memory design follows the module contract: the
// flag data is initialized into the shared memory once (by the first
// instantiation's start function) and instances are thin execution contexts.
// All entry into WASM is serialized by one mutex, so the module's internal
// de-duplication and flush locks never contend and memory.atomic.wait never
// blocks a host goroutine.
type Host struct {
	runtime  wazero.Runtime
	instance api.Module
	memory   api.Memory
	mod      *Module
	closers  []api.Closer

	mu sync.Mutex // entry mutex: one WASM call at a time

	alloc  api.Function
	frees  api.Function
	ackLog api.Function

	resolveProcessStart   api.Function
	resolveProcessResume  api.Function
	resolveProcessDiscard api.Function
	applyFlags            api.Function
	flushLogs             api.Function
	evaluate              api.Function
	metricsSnapshot       api.Function
	allocatedBytes        api.Function
	observeLatency        api.Function

	logSink LogSink

	hasMetricsSnapshot bool
	hasAllocatedBytes  bool
	hasObserveLatency  bool
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
	// ApplySeen and SchemaSeen implement the exposure/schema de-duplication
	// imports: they return true when the fingerprint was already reported.
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
// module declares. The returned host is ready to resolve; Close tears the
// instance down.
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

	h := &Host{
		runtime:            r,
		mod:                mod,
		logSink:            cfg.LogSink,
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
	// with the module's declared limits. wazero allocates shared memories
	// fixed at their maximum, which is what threads semantics expect.
	limits := mod.info.HostMemoryLimits
	helperCtx := experimental.WithMemoryAllocator(ctx, lazyMemoryAllocator{})
	helper, err := r.InstantiateWithConfig(helperCtx, sharedMemoryHelperWasm(limits.Initial, limits.Maximum, limits.HasMax), wazero.NewModuleConfig().WithName("host"))
	if err != nil {
		r.Close(ctx)
		return nil, fmt.Errorf("shared memory helper: %w", err)
	}
	h.closers = append(h.closers, helper)
	h.memory = helper.Memory()
	if h.memory == nil {
		r.Close(ctx)
		return nil, fmt.Errorf("shared memory helper exported no memory")
	}

	instance, err := r.Instantiate(ctx, mod.Bytes)
	if err != nil {
		r.Close(ctx)
		return nil, err
	}
	h.instance = instance
	h.closers = append(h.closers, instance)

	h.alloc = instance.ExportedFunction("alloc")
	h.frees = instance.ExportedFunction("free")
	h.ackLog = instance.ExportedFunction("ack_logs")
	h.resolveProcessStart = instance.ExportedFunction("resolve_process_start")
	h.resolveProcessResume = instance.ExportedFunction("resolve_process_resume")
	h.resolveProcessDiscard = instance.ExportedFunction("resolve_process_discard")
	h.applyFlags = instance.ExportedFunction("apply_flags")
	h.flushLogs = instance.ExportedFunction("flush_logs")
	h.evaluate = instance.ExportedFunction("evaluate")
	h.metricsSnapshot = instance.ExportedFunction("metrics_snapshot")
	h.allocatedBytes = instance.ExportedFunction("allocated_bytes")
	h.observeLatency = instance.ExportedFunction("observe_client_resolve_latency")
	if h.alloc == nil || h.frees == nil || h.ackLog == nil ||
		h.resolveProcessStart == nil || h.resolveProcessResume == nil ||
		h.resolveProcessDiscard == nil || h.applyFlags == nil ||
		h.flushLogs == nil || h.evaluate == nil {
		h.Close()
		return nil, errMissingExport
	}
	return h, nil
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

// ack acknowledges a delivered payload. The write_logs callback runs inside a
// locked WASM call, so this reenters through the calling module's own export
// (allowed by the contract: ack_logs may be called from any thread) instead
// of taking the host entry mutex.
func (h *Host) ack(caller api.Module, payload uint64) {
	defer func() {
		// ack_logs must never throw; a failure here is logged and dropped.
		if r := recover(); r != nil {
			slog.Default().Error("ack_logs panicked", "panic", r)
		}
	}()
	ackFn := h.ackLog
	if fn := caller.ExportedFunction("ack_logs"); fn != nil {
		ackFn = fn
	}
	if ackFn == nil {
		return
	}
	if _, err := ackFn.Call(context.Background(), payload); err != nil {
		slog.Default().Error("ack_logs failed", "error", err)
	}
}

// Close tears the instance down while the shared memory is still alive, then
// releases the runtime and its modules. It must not be called concurrently
// with in-flight calls (the provider drains first).
func (h *Host) Close() {
	h.mu.Lock()
	defer h.mu.Unlock()
	ctx := context.Background()
	if h.instance != nil {
		if fn := h.instance.ExportedFunction("destroy_instance"); fn != nil {
			if _, err := fn.Call(ctx); err != nil {
				slog.Default().Error("destroy_instance failed", "error", err)
			}
		}
		h.instance = nil
	}
	// Reverse order: client instance first, then host modules/helper.
	for i := len(h.closers) - 1; i >= 0; i-- {
		_ = h.closers[i].Close(ctx)
	}
	h.closers = nil
	_ = h.runtime.Close(ctx)
}

func boolToI32(b bool) int32 {
	if b {
		return 1
	}
	return 0
}
