package local_resolver

import (
	"context"
	"encoding/binary"
	"errors"
	"sync"
	"time"

	_ "embed"

	messages "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto"
	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/resolverinternal"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/resolver"
	"github.com/tetratelabs/wazero"
	"github.com/tetratelabs/wazero/api"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/timestamppb"
)

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
	instance api.Module
	logSink  LogSink
	mu       *sync.Mutex
}

var _ LocalResolver = (*WasmResolver)(nil)

func (r *WasmResolver) SetResolverState(request *messages.SetResolverStateRequest) error {
	return r.call("wasm_msg_guest_set_resolver_state", request, nil)
}

func (r *WasmResolver) ResolveWithSticky(request *resolver.ResolveWithStickyRequest) (*resolver.ResolveWithStickyResponse, error) {
	resp := &resolver.ResolveWithStickyResponse{}
	err := r.call("wasm_msg_guest_resolve_with_sticky", request, resp)
	return resp, err
}

func (r *WasmResolver) FlushAllLogs() error {
	resp := &resolverv1.WriteFlagLogsRequest{}
	err := r.call("wasm_msg_guest_bounded_flush_logs", nil, resp)
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
		wsmMsgReq := &messages.Request{
			Data: mustMarshal(request),
		}
		reqPtr = transfer(r.instance, mustMarshal(wsmMsgReq))
	}
	ctx := context.Background()
	fn := r.instance.ExportedFunction(fnName)
	resPtr, err := fn.Call(ctx, uint64(reqPtr))
	if err != nil {
		panic(err)
	}

	if resPtr[0] != 0 {
		resBytes := consume(r.instance, uint32(resPtr[0]))
		wsmMsgRes := &messages.Response{}
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
	runtime wazero.Runtime
	module  wazero.CompiledModule
	logSink LogSink
}

var _ LocalResolverFactory = (*WasmResolverFactory)(nil)

func NewWasmResolverFactory(logSink LogSink) LocalResolverFactory {
	ctx := context.Background()
	runtime := wazero.NewRuntime(ctx)
	_, err := runtime.NewHostModuleBuilder("wasm_msg").
		NewFunctionBuilder().
		WithFunc(func(ctx context.Context, mod api.Module, ptr uint32) uint32 {
			// Return current timestamp
			now := time.Now()
			timestamp := timestamppb.New(now)

			// Create response wrapper
			response := &messages.Response{
				Result: &messages.Response_Data{
					Data: mustMarshal(timestamp),
				},
			}

			return transfer(mod, mustMarshal(response))
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

func (wrf *WasmResolverFactory) New() LocalResolver {
	ctx := context.Background()
	config := wazero.NewModuleConfig().WithName("")
	instance, err := wrf.runtime.InstantiateModule(ctx, wrf.module, config)
	if err != nil {
		panic(err)
	}
	return &WasmResolver{
		instance: instance,
		logSink:  wrf.logSink,
		mu:       &sync.Mutex{},
	}
}

func (wrf *WasmResolverFactory) Close(ctx context.Context) error {
	return wrf.runtime.Close(ctx)
}

// consume reads data from WASM memory and frees it
func consume(inst api.Module, addr uint32) []byte {
	memory := inst.Memory()

	// Read length (assuming 4-byte length prefix)
	lenBytes, ok := memory.Read(addr-4, 4)
	if !ok {
		panic("failed to read buffer len")
	}
	length := binary.LittleEndian.Uint32(lenBytes) - 4

	// Read data
	data, ok := memory.Read(addr, length)
	if !ok {
		panic("failed to read buffer")
	}

	// Make a copy of the data before freeing the WASM memory
	dataCopy := make([]byte, length)
	copy(dataCopy, data)

	// Free memory
	ctx := context.Background()
	_, err := inst.ExportedFunction("wasm_msg_free").Call(ctx, uint64(addr))
	if err != nil {
		panic(err)
	}

	return dataCopy
}

func transfer(inst api.Module, data []byte) uint32 {
	ctx := context.Background()

	// Allocate memory in WASM
	results, err := inst.ExportedFunction("wasm_msg_alloc").Call(ctx, uint64(len(data)))
	if err != nil {
		panic(err)
	}

	addr := uint32(results[0])

	// Write data to WASM memory
	memory := inst.Memory()
	memory.Write(addr, data)

	return addr
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
