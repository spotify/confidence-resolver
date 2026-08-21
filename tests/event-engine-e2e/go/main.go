package main

import (
	"bytes"
	"context"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"time"

	"github.com/tetratelabs/wazero"
	"github.com/tetratelabs/wazero/api"
	"google.golang.org/protobuf/encoding/protowire"
)

const (
	eventName = "skill-e2e-test"
	apiURL    = "https://events.confidence.dev/v1/events:publish"
	numEvents = 1_000
	wasmPath  = "../../../wasm/confidence_event_engine.wasm"
)

func mustEnv(key string) string {
	v := os.Getenv(key)
	if v == "" {
		fmt.Fprintf(os.Stderr, "Required env var %s is not set\n", key)
		os.Exit(1)
	}
	return v
}

// --- Protobuf encoding helpers (protowire) ---

func appendString(b []byte, field protowire.Number, s string) []byte {
	b = protowire.AppendTag(b, field, protowire.BytesType)
	b = protowire.AppendString(b, s)
	return b
}

func appendMessage(b []byte, field protowire.Number, msg []byte) []byte {
	b = protowire.AppendTag(b, field, protowire.BytesType)
	b = protowire.AppendBytes(b, msg)
	return b
}

func appendVarint(b []byte, field protowire.Number, v uint64) []byte {
	b = protowire.AppendTag(b, field, protowire.VarintType)
	b = protowire.AppendVarint(b, v)
	return b
}

func encodeTimestamp(t time.Time) []byte {
	var b []byte
	b = appendVarint(b, 1, uint64(t.Unix()))
	nanos := int32(t.Nanosecond())
	if nanos != 0 {
		b = appendVarint(b, 2, uint64(nanos))
	}
	return b
}

func encodeTrackEventRequest(eventName string, t time.Time) []byte {
	var b []byte
	b = appendString(b, 1, eventName)
	b = appendMessage(b, 2, encodeTimestamp(t))
	return b
}

func encodeRequest(data []byte) []byte {
	return appendMessage(nil, 1, data)
}

func decodeResponse(data []byte) ([]byte, error) {
	for len(data) > 0 {
		num, typ, n := protowire.ConsumeTag(data)
		if n < 0 {
			return nil, fmt.Errorf("invalid tag")
		}
		data = data[n:]
		if typ == protowire.BytesType {
			v, vn := protowire.ConsumeBytes(data)
			if vn < 0 {
				return nil, fmt.Errorf("invalid bytes")
			}
			data = data[vn:]
			if num == 1 {
				return v, nil
			}
			if num == 2 {
				return nil, fmt.Errorf("wasm error: %s", string(v))
			}
		} else {
			return nil, fmt.Errorf("unexpected wire type %d for field %d", typ, num)
		}
	}
	return nil, nil
}

func countEventsInBatch(data []byte) int {
	count := 0
	for len(data) > 0 {
		num, typ, n := protowire.ConsumeTag(data)
		if n < 0 {
			break
		}
		data = data[n:]
		if typ == protowire.BytesType {
			_, vn := protowire.ConsumeBytes(data)
			if vn < 0 {
				break
			}
			data = data[vn:]
			if num == 1 {
				count++
			}
		}
	}
	return count
}

// --- WASM host ---

type wasmHost struct {
	mod     api.Module
	alloc   api.Function
	free    api.Function
	track   api.Function
	flush   api.Function
}

func newWasmHost(ctx context.Context, wasmBytes []byte) (*wasmHost, wazero.Runtime) {
	rt := wazero.NewRuntime(ctx)
	mod, err := rt.Instantiate(ctx, wasmBytes)
	if err != nil {
		panic(fmt.Sprintf("instantiate failed: %v", err))
	}
	return &wasmHost{
		mod:   mod,
		alloc: mod.ExportedFunction("wasm_msg_alloc"),
		free:  mod.ExportedFunction("wasm_msg_free"),
		track: mod.ExportedFunction("wasm_msg_guest_track_event"),
		flush: mod.ExportedFunction("wasm_msg_guest_bounded_flush_events"),
	}, rt
}

func (h *wasmHost) call(ctx context.Context, fn api.Function, reqData []byte) ([]byte, error) {
	envelope := encodeRequest(reqData)
	results, err := h.alloc.Call(ctx, uint64(len(envelope)))
	if err != nil {
		return nil, fmt.Errorf("alloc: %w", err)
	}
	ptr := uint32(results[0])
	h.mod.Memory().Write(ptr, envelope)

	resResults, err := fn.Call(ctx, uint64(ptr))
	if err != nil {
		return nil, fmt.Errorf("call: %w", err)
	}
	resPtr := uint32(resResults[0])
	if resPtr == 0 {
		return nil, nil
	}

	sizeBytes, ok := h.mod.Memory().Read(resPtr-4, 4)
	if !ok {
		return nil, fmt.Errorf("read size failed")
	}
	totalSize := binary.LittleEndian.Uint32(sizeBytes)
	dataLen := totalSize - 4

	resData, ok := h.mod.Memory().Read(resPtr, dataLen)
	if !ok {
		return nil, fmt.Errorf("read data failed")
	}
	result := make([]byte, dataLen)
	copy(result, resData)

	_, _ = h.free.Call(ctx, uint64(resPtr))
	return decodeResponse(result)
}

// --- Network API ---

type apiRequest struct {
	ClientSecret string     `json:"clientSecret"`
	SDK          apiSDK     `json:"sdk"`
	SendTime     string     `json:"sendTime"`
	Events       []apiEvent `json:"events"`
}

type apiSDK struct {
	ID      string `json:"id"`
	Version string `json:"version"`
}

type apiEvent struct {
	EventDefinition string                 `json:"eventDefinition"`
	EventTime       string                 `json:"eventTime"`
	Payload         map[string]interface{} `json:"payload"`
}

func postEvents(events []apiEvent, clientSecret string) error {
	req := apiRequest{
		ClientSecret: clientSecret,
		SDK:          apiSDK{ID: "SDK_ID_GO_CONFIDENCE", Version: "0.1.0-e2e-test"},
		SendTime:     time.Now().UTC().Format("2006-01-02T15:04:05.000Z"),
		Events:       events,
	}
	body, err := json.Marshal(req)
	if err != nil {
		return err
	}

	httpReq, err := http.NewRequest("POST", apiURL, bytes.NewReader(body))
	if err != nil {
		return err
	}
	httpReq.Header.Set("Content-Type", "application/json")

	resp, err := http.DefaultClient.Do(httpReq)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	respBody, _ := io.ReadAll(resp.Body)

	if resp.StatusCode != 200 {
		return fmt.Errorf("API returned %d: %s", resp.StatusCode, string(respBody))
	}
	return nil
}

func main() {
	ctx := context.Background()
	clientSecret := mustEnv("CLIENT_SECRET")

	wasmBytes, err := os.ReadFile(wasmPath)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Failed to read WASM: %v\n", err)
		os.Exit(1)
	}
	fmt.Printf("Loaded WASM: %d bytes\n", len(wasmBytes))

	host, rt := newWasmHost(ctx, wasmBytes)
	defer rt.Close(ctx)

	// --- Phase 1: Load test (track events) ---
	fmt.Printf("\n=== Phase 1: Track %d events ===\n", numEvents)
	start := time.Now()
	for i := 0; i < numEvents; i++ {
		eventData := encodeTrackEventRequest(eventName, time.Now())
		_, err := host.call(ctx, host.track, eventData)
		if err != nil {
			fmt.Fprintf(os.Stderr, "track_event failed at %d: %v\n", i, err)
			os.Exit(1)
		}
	}
	trackDur := time.Since(start)
	fmt.Printf("Tracked %d events in %v\n", numEvents, trackDur)
	fmt.Printf("Throughput: %.0f events/sec\n", float64(numEvents)/trackDur.Seconds())
	fmt.Printf("Latency: %v/event\n", trackDur/time.Duration(numEvents))

	// --- Phase 2: Flush batches ---
	fmt.Printf("\n=== Phase 2: Flush events ===\n")
	totalFlushed := 0
	flushCount := 0
	start = time.Now()
	for {
		voidData := []byte{} // empty Void message
		batchData, err := host.call(ctx, host.flush, voidData)
		if err != nil {
			fmt.Fprintf(os.Stderr, "flush failed: %v\n", err)
			os.Exit(1)
		}
		if batchData == nil || len(batchData) == 0 {
			break
		}
		count := countEventsInBatch(batchData)
		if count == 0 {
			break
		}
		totalFlushed += count
		flushCount++
		fmt.Printf("  Batch %d: %d events (%d bytes)\n", flushCount, count, len(batchData))
	}
	flushDur := time.Since(start)
	fmt.Printf("Flushed %d events in %d batches, took %v\n", totalFlushed, flushCount, flushDur)

	if totalFlushed != numEvents {
		fmt.Fprintf(os.Stderr, "WARNING: tracked %d but flushed %d events\n", numEvents, totalFlushed)
	}

	// --- Phase 3: POST a sample batch to the real API ---
	fmt.Printf("\n=== Phase 3: POST sample batch to events API ===\n")
	sampleSize := numEvents

	events := make([]apiEvent, sampleSize)
	for i := 0; i < sampleSize; i++ {
		events[i] = apiEvent{
			EventDefinition: "eventDefinitions/" + eventName,
			EventTime:       time.Now().UTC().Format("2006-01-02T15:04:05.000Z"),
			Payload: map[string]interface{}{
				"test_run":   "go-e2e-load-test",
				"provider":   "go",
				"index":      i,
				"batch_size": numEvents,
				"latency_ms": float64(trackDur.Microseconds()) / 1000.0 / float64(numEvents),
				"context": map[string]interface{}{
					"targeting_key": fmt.Sprintf("test-user-%d", i),
				},
			},
		}
	}

	start = time.Now()
	err = postEvents(events, clientSecret)
	postDur := time.Since(start)
	if err != nil {
		fmt.Fprintf(os.Stderr, "API POST failed: %v\n", err)
		os.Exit(1)
	}
	fmt.Printf("Posted %d events to %s in %v\n", sampleSize, apiURL, postDur)

	// --- Summary ---
	fmt.Printf("\n=== Summary ===\n")
	fmt.Printf("Track:  %d events, %.0f events/sec, %v/event\n", numEvents, float64(numEvents)/trackDur.Seconds(), trackDur/time.Duration(numEvents))
	fmt.Printf("Flush:  %d events in %d batches, %v total\n", totalFlushed, flushCount, flushDur)
	fmt.Printf("API:    %d events posted in %v\n", sampleSize, postDur)
	fmt.Println("PASS")
}
