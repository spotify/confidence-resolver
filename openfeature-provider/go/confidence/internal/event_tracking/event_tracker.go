// Package event_tracking provides a WASM-based event engine for tracking
// and flushing Confidence events via the wasm-msg protocol.
package event_tracking

import (
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	"sync"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/eventswasm"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
	"github.com/tetratelabs/wazero"
	"github.com/tetratelabs/wazero/api"
	"google.golang.org/protobuf/proto"
)

var requiredExports = []string{
	"wasm_msg_alloc",
	"wasm_msg_free",
	"wasm_msg_guest_track_event",
	"wasm_msg_guest_bounded_flush_events",
}

// errWasmFatal marks errors that mean the WASM instance may be in an undefined
// state (a trap, or a failed memory/alloc operation) and must be reloaded.
// Client-side errors — proto marshal/unmarshal failures, or an error cleanly
// reported by the guest — are not fatal: the instance is still healthy and its
// buffered events must be preserved.
var errWasmFatal = errors.New("wasm instance is unusable")

func fatalf(format string, args ...any) error {
	return fmt.Errorf("%w: "+format, append([]any{errWasmFatal}, args...)...)
}

// EventTracker wraps a WASM event engine instance and exposes TrackEvent and
// FlushEvents operations using the wasm-msg protocol. On a WASM trap the
// instance is transparently reloaded so the provider keeps functioning —
// buffered events in the crashed instance are lost, mirroring how
// RecoveringResolver handles the flag resolver.
type EventTracker struct {
	runtime  wazero.Runtime
	module   wazero.CompiledModule
	instance api.Module
	mu       sync.Mutex
	closed   bool
}

// NewEventTracker compiles and instantiates the event engine WASM module.
// The event engine has no host imports, so no host module is registered.
// If useInterpreter is true, wazero's interpreter mode is used instead of JIT.
func NewEventTracker(wasmBytes []byte, useInterpreter bool) (*EventTracker, error) {
	ctx := context.Background()

	var runtime wazero.Runtime
	if useInterpreter {
		runtime = wazero.NewRuntimeWithConfig(ctx, wazero.NewRuntimeConfigInterpreter())
	} else {
		runtime = wazero.NewRuntime(ctx)
	}

	module, err := runtime.CompileModule(ctx, wasmBytes)
	if err != nil {
		runtime.Close(ctx)
		return nil, fmt.Errorf("failed to compile event engine WASM: %w", err)
	}

	tracker := &EventTracker{runtime: runtime, module: module}
	instance, err := tracker.newInstance(ctx)
	if err != nil {
		runtime.Close(ctx)
		return nil, err
	}
	tracker.instance = instance
	return tracker, nil
}

// newInstance instantiates the compiled module and verifies required exports.
func (t *EventTracker) newInstance(ctx context.Context) (api.Module, error) {
	instance, err := t.runtime.InstantiateModule(ctx, t.module, wazero.NewModuleConfig().WithName(""))
	if err != nil {
		return nil, fmt.Errorf("failed to instantiate event engine WASM: %w", err)
	}
	for _, name := range requiredExports {
		if instance.ExportedFunction(name) == nil {
			instance.Close(ctx)
			return nil, fmt.Errorf("event engine WASM missing required export: %s", name)
		}
	}
	return instance, nil
}

// TrackEvent sends a track event request to the WASM event engine.
// The event is buffered internally; call FlushEvents to retrieve the batch.
func (t *EventTracker) TrackEvent(request *eventswasm.TrackEventRequest) error {
	return t.call("wasm_msg_guest_track_event", request, nil)
}

// FlushEvents retrieves all buffered events from the WASM event engine.
// Returns a FlushEventsResponse containing the batch of events ready for
// network transmission. The caller is responsible for wrapping them in a
// confidence.events.v1.PublishEventsRequest (adding client_secret, sdk info
// and send_time) before publishing.
func (t *EventTracker) FlushEvents() (*eventswasm.FlushEventsResponse, error) {
	resp := &eventswasm.FlushEventsResponse{}
	err := t.call("wasm_msg_guest_bounded_flush_events", nil, resp)
	return resp, err
}

// Close releases the WASM instance and runtime resources.
func (t *EventTracker) Close() error {
	t.mu.Lock()
	defer t.mu.Unlock()

	if t.closed {
		return nil
	}
	t.closed = true

	ctx := context.Background()
	var instanceErr, runtimeErr error
	if t.instance != nil {
		instanceErr = t.instance.Close(ctx)
	}
	if t.runtime != nil {
		runtimeErr = t.runtime.Close(ctx)
	}
	return errors.Join(instanceErr, runtimeErr)
}

// call implements the wasm-msg protocol: marshal request into a Request envelope,
// allocate WASM memory, write, call the export, read the Response envelope, free.
// On a WASM trap the instance is reloaded and the error returned; the next call
// runs against the fresh instance.
func (t *EventTracker) call(fnName string, request proto.Message, response proto.Message) error {
	t.mu.Lock()
	defer t.mu.Unlock()

	if t.closed {
		return errors.New("event tracker is closed")
	}

	err := t.callLocked(fnName, request, response)
	if errors.Is(err, errWasmFatal) {
		// The instance may be in an undefined state; reload it. Buffered events
		// in the crashed instance are lost, as with RecoveringResolver.
		t.reloadLocked()
	}
	return err
}

// reloadLocked replaces a trapped WASM instance with a fresh one.
// Buffered events in the old instance are lost. Caller must hold t.mu.
func (t *EventTracker) reloadLocked() {
	ctx := context.Background()
	if t.instance != nil {
		_ = t.instance.Close(ctx)
		t.instance = nil
	}
	instance, err := t.newInstance(ctx)
	if err != nil {
		// Leave instance nil — subsequent calls fail fast until Close.
		return
	}
	t.instance = instance
}

func (t *EventTracker) callLocked(fnName string, request proto.Message, response proto.Message) error {
	if t.instance == nil {
		return errors.New("event tracker has no live WASM instance")
	}

	reqPtr := uint32(0)
	if request != nil {
		innerBytes, err := proto.Marshal(request)
		if err != nil {
			return fmt.Errorf("failed to marshal request: %w", err)
		}
		envelopeBytes, err := proto.Marshal(&wasm.Request{Data: innerBytes})
		if err != nil {
			return fmt.Errorf("failed to marshal request envelope: %w", err)
		}
		reqPtr, err = t.allocAndWrite(envelopeBytes)
		if err != nil {
			return err
		}
	}

	ctx := context.Background()
	fn := t.instance.ExportedFunction(fnName)
	if fn == nil {
		return fatalf("exported function %s not found", fnName)
	}

	resPtr, err := fn.Call(ctx, uint64(reqPtr))
	if err != nil {
		return fatalf("WASM call %s failed: %v", fnName, err)
	}

	if resPtr[0] == 0 {
		return nil
	}

	resBytes, err := t.readAndFree(uint32(resPtr[0]))
	if err != nil {
		return err
	}
	resEnvelope := &wasm.Response{}
	if err := proto.Unmarshal(resBytes, resEnvelope); err != nil {
		return fmt.Errorf("failed to unmarshal response envelope: %w", err)
	}
	if errMsg := resEnvelope.GetError(); errMsg != "" {
		return errors.New(errMsg)
	}
	if response != nil {
		if err := proto.Unmarshal(resEnvelope.GetData(), response); err != nil {
			return fmt.Errorf("failed to unmarshal response: %w", err)
		}
	}
	return nil
}

// allocAndWrite allocates WASM memory and writes data into it.
func (t *EventTracker) allocAndWrite(data []byte) (uint32, error) {
	ctx := context.Background()
	results, err := t.instance.ExportedFunction("wasm_msg_alloc").Call(ctx, uint64(len(data)))
	if err != nil {
		return 0, fatalf("wasm_msg_alloc failed: %v", err)
	}
	addr := uint32(results[0])
	if !t.instance.Memory().Write(addr, data) {
		return 0, fatalf("failed to write request into WASM memory")
	}
	return addr, nil
}

// readAndFree reads data from WASM memory and frees the allocation.
// The wasm-msg protocol stores a 4-byte little-endian length prefix at addr-4,
// where the length includes the 4-byte prefix itself.
func (t *EventTracker) readAndFree(addr uint32) ([]byte, error) {
	memory := t.instance.Memory()

	lenBytes, ok := memory.Read(addr-4, 4)
	if !ok {
		return nil, fatalf("failed to read buffer length from WASM memory")
	}
	length := binary.LittleEndian.Uint32(lenBytes) - 4

	data, ok := memory.Read(addr, length)
	if !ok {
		return nil, fatalf("failed to read buffer data from WASM memory")
	}
	dataCopy := make([]byte, length)
	copy(dataCopy, data)

	ctx := context.Background()
	if _, err := t.instance.ExportedFunction("wasm_msg_free").Call(ctx, uint64(addr)); err != nil {
		return nil, fatalf("wasm_msg_free failed: %v", err)
	}
	return dataCopy, nil
}
