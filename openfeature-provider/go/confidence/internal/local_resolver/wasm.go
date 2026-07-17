package local_resolver

import (
	"context"
	_ "embed"
	"encoding/binary"
	"errors"
	"fmt"
	"log/slog"
	"sync"
	"sync/atomic"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	"github.com/tetratelabs/wazero"
	"github.com/tetratelabs/wazero/api"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/timestamppb"
)

// instanceCounter is a package-level counter for auto-assigning unique instance IDs.
var instanceCounter atomic.Int64

// defaultWasmBytes contains the embedded WASM resolver module.
// This file is automatically populated during the build process from wasm/confidence_resolver.wasm.
// The WASM file is built from the Rust source in wasm/rust-guest/ and must be kept in sync.
//
// CI validates that this embedded file matches the built WASM to prevent version drift.

//go:embed assets/confidence_resolver.wasm
var wasmBytes []byte

type LogSink func(logs *resolverv1.WriteFlagLogsRequest)

func NoOpLogSink(logs *resolverv1.WriteFlagLogsRequest) {}

type WasmResolver struct {
	instance   api.Module
	logSink    LogSink
	mu         *sync.Mutex
	instanceID string
	fnCache    sync.Map
	initLabels map[string]string
	firstFlush bool
}

var _ LocalResolver = (*WasmResolver)(nil)

// exportedFunction returns a cached WASM function handle. Wazevo allocates a
// new call engine on every Module.ExportedFunction call; caching avoids that
// on the hot path.
func (r *WasmResolver) exportedFunction(name string) api.Function {
	if fn, ok := r.fnCache.Load(name); ok {
		return fn.(api.Function)
	}
	fn := r.instance.ExportedFunction(name)
	r.fnCache.Store(name, fn)
	return fn
}

func (r *WasmResolver) SetResolverState(request *wasm.SetResolverStateRequest) error {
	return r.call("wasm_msg_guest_set_resolver_state", request, nil)
}

func (r *WasmResolver) ResolveProcess(request *wasm.ResolveProcessRequest) (*wasm.ResolveProcessResponse, error) {
	resp := &wasm.ResolveProcessResponse{}
	err := r.call("wasm_msg_guest_resolve_flags", request, resp)
	return resp, err
}

func (r *WasmResolver) RegisterResolve(request *wasm.RegisterResolveRequest) {
	if err := r.call("wasm_msg_guest_register_resolve", request, nil); err != nil {
		slog.Warn("Failed to register resolve telemetry", "error", err)
	}
}

func (r *WasmResolver) ApplyFlags(request *resolver.ApplyFlagsRequest) error {
	if err := r.call("wasm_msg_guest_apply_flags", request, nil); err != nil {
		// Apply is best-effort logging — surface the failure but don't propagate.
		slog.Warn("Failed to apply flags", "error", err)
	}
	return nil
}

func (r *WasmResolver) FlushAllLogs() error {
	resp := &resolverv1.WriteFlagLogsRequest{}
	err := r.call("wasm_msg_guest_bounded_flush_logs", nil, resp)
	if err == nil && r.firstFlush {
		r.firstFlush = false
		if resp.TelemetryData == nil {
			resp.TelemetryData = &resolverv1.TelemetryData{}
		}
		resp.TelemetryData.ProviderInitRate = append(
			resp.TelemetryData.ProviderInitRate,
			&resolverv1.TelemetryData_ProviderInitRate{Count: 1, Labels: r.initLabels},
		)
	}
	if err == nil && proto.Size(resp) > 0 {
		r.logSink(resp)
	}
	return err
}

func (r *WasmResolver) FlushAssignLogs() error {
	resp := &resolverv1.WriteFlagLogsRequest{}
	err := r.call("wasm_msg_guest_bounded_flush_assign", nil, resp)
	if err == nil && len(resp.FlagAssigned) > 0 {
		r.logSink(resp)
	}
	return err
}

func (r *WasmResolver) PrometheusSnapshot(bucketsPerDecade uint32, openmetrics bool) string {
	req := &wasm.PrometheusSnapshotRequest{
		Instance:         r.instanceID,
		BucketsPerDecade: bucketsPerDecade,
		Openmetrics:      openmetrics,
	}
	resp := &wasm.PrometheusSnapshotResponse{}
	if err := r.call("wasm_msg_guest_prometheus_snapshot", req, resp); err != nil {
		slog.Warn("prometheus snapshot failed", "error", err)
		return ""
	}
	return resp.GetText()
}

func (r *WasmResolver) Close(ctx context.Context) error {
	// TODO we should call flush assigned until it doesn't flush any more
	r.FlushAllLogs()
	return r.instance.Close(ctx)
}

func (r *WasmResolver) call(fnName string, request proto.Message, response proto.Message) error {
	r.mu.Lock()
	defer r.mu.Unlock()

	reqPtr := uint32(0)
	if request != nil {
		wsmMsgReq := &wasm.Request{
			Data: mustMarshal(request),
		}
		reqPtr = r.transfer(mustMarshal(wsmMsgReq))
	}
	ctx := context.Background()
	fn := r.exportedFunction(fnName)
	resPtr, err := fn.Call(ctx, uint64(reqPtr))
	if err != nil {
		panic(err)
	}

	if resPtr[0] != 0 {
		resBytes := r.consume(uint32(resPtr[0]))
		wsmMsgRes := &wasm.Response{}
		mustUnmarshal(resBytes, wsmMsgRes)
		errMsg := wsmMsgRes.GetError()
		if errMsg != "" {
			return errors.New(errMsg)
		}
		if response != nil {
			return proto.Unmarshal(wsmMsgRes.GetData(), response)
		}
	}
	return nil
}

type WasmResolverFactory struct {
	runtime    wazero.Runtime
	module     wazero.CompiledModule
	logSink    LogSink
	initLabels map[string]string
}

var _ LocalResolverFactory = (*WasmResolverFactory)(nil)

func consumeRequest(inst api.Module, ptr uint32) []byte {
	if ptr == 0 {
		return nil
	}
	data := consume(inst, ptr)
	req := &wasm.Request{}
	mustUnmarshal(data, req)
	return req.GetData()
}

func transferResponseSuccess(inst api.Module, data []byte) uint32 {
	resp := &wasm.Response{Result: &wasm.Response_Data{Data: data}}
	return transfer(inst, mustMarshal(resp))
}

func transferResponseError(inst api.Module, errMsg string) uint32 {
	resp := &wasm.Response{Result: &wasm.Response_Error{Error: errMsg}}
	return transfer(inst, mustMarshal(resp))
}

func NewWasmResolverFactory(logSink LogSink) LocalResolverFactory {
	ctx := context.Background()
	runtime := wazero.NewRuntime(ctx)
	_, err := runtime.NewHostModuleBuilder("wasm_msg").
		NewFunctionBuilder().
		WithFunc(func(ctx context.Context, mod api.Module, ptr uint32) uint32 {
			consumeRequest(mod, ptr)
			return transferResponseSuccess(mod, mustMarshal(timestamppb.Now()))
		}).
		Export("wasm_msg_host_current_time").
		Instantiate(ctx)
	if err != nil {
		runtime.Close(ctx)
		panic(err)
	}
	module, err := runtime.CompileModule(ctx, wasmBytes)
	if err != nil {
		runtime.Close(ctx)
		panic(err)
	}
	return &WasmResolverFactory{
		runtime: runtime,
		module:  module,
		logSink: logSink,
	}
}

func NewWasmResolverFactoryWithLabels(logSink LogSink, initLabels map[string]string) LocalResolverFactory {
	factory := NewWasmResolverFactory(logSink).(*WasmResolverFactory)
	factory.initLabels = initLabels
	return factory
}

func (wrf *WasmResolverFactory) New() LocalResolver {
	ctx := context.Background()
	config := wazero.NewModuleConfig().WithName("")
	instance, err := wrf.runtime.InstantiateModule(ctx, wrf.module, config)
	if err != nil {
		panic(err)
	}
	id := instanceCounter.Add(1)
	return &WasmResolver{
		instance:   instance,
		logSink:    wrf.logSink,
		mu:         &sync.Mutex{},
		instanceID: fmt.Sprintf("%d", id),
		initLabels: wrf.initLabels,
		firstFlush: true,
	}
}

func (wrf *WasmResolverFactory) Close(ctx context.Context) error {
	return wrf.runtime.Close(ctx)
}

// readAndFree and allocAndWrite are the low-level WASM memory primitives.
// They accept the alloc/free function handle as a parameter so callers can
// choose between a cached handle (WasmResolver methods, hot path) and a
// direct ExportedFunction lookup (host callbacks, cold path).
func readAndFree(inst api.Module, addr uint32, freeFn api.Function) []byte {
	memory := inst.Memory()

	lenBytes, ok := memory.Read(addr-4, 4)
	if !ok {
		panic("failed to read buffer len")
	}
	length := binary.LittleEndian.Uint32(lenBytes) - 4

	data, ok := memory.Read(addr, length)
	if !ok {
		panic("failed to read buffer")
	}

	dataCopy := make([]byte, length)
	copy(dataCopy, data)

	ctx := context.Background()
	_, err := freeFn.Call(ctx, uint64(addr))
	if err != nil {
		panic(err)
	}

	return dataCopy
}

func allocAndWrite(inst api.Module, data []byte, allocFn api.Function) uint32 {
	ctx := context.Background()

	results, err := allocFn.Call(ctx, uint64(len(data)))
	if err != nil {
		panic(err)
	}

	addr := uint32(results[0])
	inst.Memory().Write(addr, data)

	return addr
}

// Method versions use cached function handles (hot path — called per resolve).
func (r *WasmResolver) consume(addr uint32) []byte {
	return readAndFree(r.instance, addr, r.exportedFunction("wasm_msg_free"))
}

func (r *WasmResolver) transfer(data []byte) uint32 {
	return allocAndWrite(r.instance, data, r.exportedFunction("wasm_msg_alloc"))
}

// Free-function versions look up the handle each time (cold path — host callbacks only).
func consume(inst api.Module, addr uint32) []byte {
	return readAndFree(inst, addr, inst.ExportedFunction("wasm_msg_free"))
}

func transfer(inst api.Module, data []byte) uint32 {
	return allocAndWrite(inst, data, inst.ExportedFunction("wasm_msg_alloc"))
}

// mustMarshal is a helper function that panics on marshal errors
func mustMarshal(message proto.Message) []byte {
	data, err := proto.Marshal(message)
	if err != nil {
		panic(err)
	}
	return data
}

func mustUnmarshal(data []byte, target proto.Message) {
	if err := proto.Unmarshal(data, target); err != nil {
		panic(err)
	}
}
