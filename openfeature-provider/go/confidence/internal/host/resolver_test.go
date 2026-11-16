package host

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"sync"
	"sync/atomic"
	"testing"

	"github.com/tetratelabs/wazero"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/structpb"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/s2wpb/resolverpb"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/s2wpb/typespb"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/s2wpb/wasmpb"
)

const testSecret = "test-secret"

type fakeStore struct {
	records []*wasmpb.Materialization_Record
	reads   atomic.Int32
	written []*wasmpb.Materialization_Record
	mu      sync.Mutex
}

func (s *fakeStore) Read(_ context.Context, req []*wasmpb.Materialization_Record) ([]*wasmpb.Materialization_Record, error) {
	s.reads.Add(1)
	return s.records, nil
}

func (s *fakeStore) Write(_ context.Context, w []*wasmpb.Materialization_Record) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.written = append(s.written, w...)
	return nil
}

func newResolver(t *testing.T, interp bool, cfg Config) *Resolver {
	t.Helper()
	r, err := NewResolver(context.Background(), fixture(t, "basic.wasm"), interp, cfg)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = r.Close(context.Background()) })
	return r
}

func bothEngines(t *testing.T, f func(t *testing.T, interp bool)) {
	for _, interp := range []bool{false, true} {
		name := map[bool]string{false: "compiler", true: "interpreter"}[interp]
		t.Run(name, func(t *testing.T) { f(t, interp) })
	}
}

func request(unit string, flags ...string) *resolverpb.ResolveFlagsRequest {
	ctx, _ := structpb.NewStruct(map[string]any{"targeting_key": unit})
	return &resolverpb.ResolveFlagsRequest{Flags: flags, EvaluationContext: ctx, ClientSecret: testSecret, Apply: true}
}

func evaluate(t *testing.T, r *Resolver, req *resolverpb.ResolveFlagsRequest, store MaterializationStore, evals ...*wasmpb.EvaluateRequest) []*wasmpb.ResolutionDetails {
	t.Helper()
	details, err := r.Evaluate(context.Background(), req, evals, store)
	if err != nil {
		t.Fatal(err)
	}
	return details
}

func TestEvaluateStruct(t *testing.T) {
	bothEngines(t, func(t *testing.T, interp bool) {
		r := newResolver(t, interp, Config{})
		def, _ := structpb.NewValue(map[string]any{"enabled": false, "message": "default"})
		d := evaluate(t, r, request("u1", "flags/plain"), &fakeStore{},
			&wasmpb.EvaluateRequest{FlagKey: "plain", DefaultValue: def},
			&wasmpb.EvaluateRequest{FlagKey: "plain.nested.label", DefaultValue: structpb.NewStringValue("x")},
			&wasmpb.EvaluateRequest{FlagKey: "plain.count", DefaultValue: structpb.NewNumberValue(0),
				ExpectedSchema: &typespb.FlagSchema{SchemaType: &typespb.FlagSchema_IntSchema{IntSchema: &typespb.FlagSchema_IntFlagSchema{}}}},
			&wasmpb.EvaluateRequest{FlagKey: "plain.message", DefaultValue: structpb.NewBoolValue(true)},
		)
		whole := d[0].GetValue().GetStructValue().AsMap()
		if whole["enabled"] != true || whole["message"] != "hello" || whole["count"] != 42.0 {
			t.Errorf("whole flag = %v", whole)
		}
		if d[0].GetVariant() != "flags/plain/variants/on" || d[0].GetReason() != resolverpb.ResolveReason_RESOLVE_REASON_MATCH {
			t.Errorf("variant/reason = %q/%v", d[0].GetVariant(), d[0].GetReason())
		}
		if got := d[1].GetValue().GetStringValue(); got != "inner" {
			t.Errorf("nested.label = %q", got)
		}
		if got := d[2].GetValue().GetNumberValue(); got != 42 {
			t.Errorf("count = %v (%v)", got, d[2].GetErrorCode())
		}
		if d[3].GetErrorCode() != wasmpb.ErrorCode_ERROR_CODE_TYPE_MISMATCH || !d[3].GetValue().GetBoolValue() {
			t.Errorf("bool default for string field = %v, %v", d[3].GetErrorCode(), d[3].GetValue())
		}
	})
}

var stickyRecord = &wasmpb.Materialization_Record{
	Unit: "u1", Materialization: "materializedSegments/sticky",
	Rule: "flags/sticky/rules/paused", Variant: "flags/sticky/variants/sticky",
}

func TestSticky(t *testing.T) {
	bothEngines(t, func(t *testing.T, interp bool) {
		r := newResolver(t, interp, Config{Instances: 2})
		eval := &wasmpb.EvaluateRequest{FlagKey: "sticky.value", DefaultValue: structpb.NewNumberValue(-1)}
		for _, tc := range []struct {
			name    string
			records []*wasmpb.Materialization_Record
			want    float64
			variant string
		}{
			{"with record", []*wasmpb.Materialization_Record{stickyRecord}, 99.99, "flags/sticky/variants/sticky"},
			{"without record", nil, 1.0, "flags/sticky/variants/fresh"},
		} {
			t.Run(tc.name, func(t *testing.T) {
				store := &fakeStore{records: tc.records}
				d := evaluate(t, r, request("u1", "flags/sticky"), store, eval)[0]
				if store.reads.Load() != 1 {
					t.Errorf("store reads = %d, want 1", store.reads.Load())
				}
				if d.GetValue().GetNumberValue() != tc.want || d.GetVariant() != tc.variant {
					t.Errorf("got %v %q (%v %s), want %v %q", d.GetValue(), d.GetVariant(), d.GetErrorCode(), d.GetErrorMessage(), tc.want, tc.variant)
				}
			})
		}
	})
}

func TestUnknownClient(t *testing.T) {
	r := newResolver(t, false, Config{})
	req := request("u1", "flags/plain")
	req.ClientSecret = "nope"
	_, err := r.Evaluate(context.Background(), req, nil, &fakeStore{})
	var re *ResolveError
	if !errors.As(err, &re) || re.Code != wasmpb.ResolveProcessResponse_Error_UNAUTHENTICATED {
		t.Fatalf("err = %v", err)
	}
}

type logSink struct {
	mu       sync.Mutex
	batches  int
	assigned int
}

func (s *logSink) write(b []byte) {
	var req wasmpb.WriteFlagLogsRequest
	if err := proto.Unmarshal(b, &req); err != nil {
		panic(err)
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.batches++
	for _, fa := range req.GetFlagAssigned() {
		s.assigned += len(fa.GetFlags())
	}
}

func TestConcurrentResolvesAndLogs(t *testing.T) {
	bothEngines(t, func(t *testing.T, interp bool) {
		sink := &logSink{}
		r := newResolver(t, interp, Config{Instances: 4, WriteLogs: sink.write})
		const goroutines, each = 8, 200
		eval := &wasmpb.EvaluateRequest{FlagKey: "plain.enabled", DefaultValue: structpb.NewBoolValue(false)}
		var wg sync.WaitGroup
		var failures atomic.Int32
		for g := range goroutines {
			wg.Add(1)
			go func() {
				defer wg.Done()
				for i := range each {
					d, err := r.Evaluate(context.Background(), request(fmt.Sprintf("u-%d-%d", g, i), "flags/plain"), []*wasmpb.EvaluateRequest{eval}, &fakeStore{})
					if err != nil || !d[0].GetValue().GetBoolValue() {
						failures.Add(1)
					}
				}
			}()
		}
		wg.Wait()
		if failures.Load() != 0 {
			t.Fatalf("%d failed evaluations", failures.Load())
		}
		if err := r.FlushLogs(context.Background()); err != nil {
			t.Fatal(err)
		}
		if sink.batches == 0 || sink.assigned != goroutines*each {
			t.Errorf("logs: %d batches, %d assigned flags, want %d", sink.batches, sink.assigned, goroutines*each)
		}
	})
}

func TestCloseFlushesAndRejects(t *testing.T) {
	sink := &logSink{}
	r, err := NewResolver(context.Background(), fixture(t, "basic.wasm"), false, Config{WriteLogs: sink.write})
	if err != nil {
		t.Fatal(err)
	}
	evaluate(t, r, request("u1", "flags/plain"), &fakeStore{})
	if err := r.Close(context.Background()); err != nil {
		t.Fatal(err)
	}
	if sink.assigned != 1 {
		t.Errorf("close flushed %d assigned flags, want 1", sink.assigned)
	}
	if _, err := r.Evaluate(context.Background(), request("u1", "flags/plain"), nil, &fakeStore{}); !errors.Is(err, ErrClosed) {
		t.Errorf("after close: %v", err)
	}
}

func TestMemoryCapBelowMinimum(t *testing.T) {
	_, err := NewResolver(context.Background(), fixture(t, "basic.wasm"), false, Config{MaxMemoryBytes: 1})
	if err == nil {
		t.Fatal("expected error for a cap below the module minimum")
	}
}

// wazero instantiates an unshared memory import against a shared memory, so Compile must reject it.
func TestUnsharedMemoryImport(t *testing.T) {
	ctx := context.Background()
	rt := NewRuntime(ctx, false)
	defer rt.Close(ctx)
	if _, err := rt.r.InstantiateWithConfig(ctx, sharedMemoryModule(1, 2), wazero.NewModuleConfig().WithName("host")); err != nil {
		t.Fatal(err)
	}
	if _, err := rt.r.Instantiate(ctx, memoryImportModule("host", "memory", false)); err != nil {
		t.Fatalf("wazero now rejects it (%v); the import parser may be unnecessary", err)
	}
	for shared, want := range map[bool]bool{true: true, false: false} {
		got, err := memoryImportShared(memoryImportModule("host", "memory", shared))
		if err != nil || got != want {
			t.Errorf("memoryImportShared(shared=%v) = %v, %v", shared, got, err)
		}
	}
	if shared, err := memoryImportShared(fixture(t, "basic.wasm")); err != nil || !shared {
		t.Errorf("fixture: shared=%v err=%v", shared, err)
	}
}

func TestDedupeFilterSharedAcrossResolvers(t *testing.T) {
	ctx := context.Background()
	filter, err := NewDedupeFilter(ctx, 8, false)
	if err != nil {
		t.Fatal(err)
	}
	defer filter.Close(ctx)
	// Two generations live at once, as during a swap; then the first retires and the second goes on.
	newGen := func() (*Resolver, *logSink) {
		sink := &logSink{}
		return newResolver(t, false, Config{WriteLogs: sink.write, ApplyFilter: filter}), sink
	}
	assigned := func(r *Resolver, sink *logSink, unit string) int {
		before := sink.assigned
		evaluate(t, r, request(unit, "flags/plain"), &fakeStore{})
		if err := r.FlushLogs(ctx); err != nil {
			t.Fatal(err)
		}
		return sink.assigned - before
	}
	old, oldSink := newGen()
	cur, curSink := newGen()
	if a, b := assigned(old, oldSink, "u1"), assigned(cur, curSink, "u1"); a != 1 || b != 0 {
		t.Errorf("u1: old generation logged %d, new %d; want 1, 0", a, b)
	}
	if err := old.Close(ctx); err != nil {
		t.Fatal(err)
	}
	if a, b := assigned(cur, curSink, "u1"), assigned(cur, curSink, "u2"); a != 0 || b != 1 {
		t.Errorf("after retiring the old generation: u1 logged %d, u2 %d; want 0, 1", a, b)
	}
}

func TestResolveReturnsResponseAndMetrics(t *testing.T) {
	r := newResolver(t, false, Config{})
	req := request("u1", "flags/plain")
	req.Apply = false
	resp, err := r.Resolve(context.Background(), req, &fakeStore{})
	if err != nil {
		t.Fatal(err)
	}
	if len(resp.GetResolvedFlags()) != 1 || resp.GetResolvedFlags()[0].GetVariant() != "flags/plain/variants/on" || len(resp.GetResolveToken()) == 0 {
		t.Errorf("response %v", resp)
	}
	for _, om := range []bool{false, true} {
		text, err := r.Metrics(context.Background(), om)
		if err != nil || !strings.Contains(text, "resolve") {
			t.Errorf("metrics (openMetrics=%v): err=%v text=%q", om, err, text)
		}
	}
	text, _ := r.Metrics(context.Background(), false)
	if !strings.Contains(text, "latency") {
		t.Errorf("no latency series after a resolve: %s", text)
	}
}
