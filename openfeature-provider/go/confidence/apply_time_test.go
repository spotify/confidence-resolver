package confidence

import (
	"bytes"
	"context"
	"errors"
	"io"
	"log/slog"
	"reflect"
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
	lr "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/local_resolver"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
	"google.golang.org/protobuf/types/known/structpb"
)

type applyTimeResolver struct {
	lr.LocalResolver
	response      *resolver.ResolveFlagsResponse
	resolveErr    error
	applyErr      error
	requests      []*resolver.ResolveFlagsRequest
	applications  []*resolver.ApplyFlagsRequest
	registrations int
}

func (r *applyTimeResolver) ResolveProcess(req *wasm.ResolveProcessRequest) (*wasm.ResolveProcessResponse, error) {
	r.requests = append(r.requests, req.GetWithoutMaterializations())
	return &wasm.ResolveProcessResponse{Result: &wasm.ResolveProcessResponse_Resolved_{
		Resolved: &wasm.ResolveProcessResponse_Resolved{Response: r.response},
	}}, r.resolveErr
}

func (r *applyTimeResolver) RegisterResolve(*wasm.RegisterResolveRequest) { r.registrations++ }

func (r *applyTimeResolver) ApplyFlags(req *resolver.ApplyFlagsRequest) error {
	r.applications = append(r.applications, req)
	return r.applyErr
}

func newApplyTimeProvider(t *testing.T) (*LocalResolverProvider, *applyTimeResolver) {
	t.Helper()
	value, err := structpb.NewStruct(map[string]any{"enabled": true, "title": "hello", "count": 3.0, "ratio": 1.5})
	if err != nil {
		t.Fatal(err)
	}
	r := &applyTimeResolver{response: &resolver.ResolveFlagsResponse{
		ResolveToken: []byte("first-resolve-token"),
		ResolvedFlags: []*resolver.ResolvedFlag{{
			Flag: "flags/example", Variant: "flags/example/variants/on", Value: value,
			Reason: resolver.ResolveReason_RESOLVE_REASON_MATCH, ShouldApply: true,
		}},
	}}
	p := newLocalResolverProvider(nil, nil, nil, "test-client", slog.New(slog.NewTextHandler(io.Discard, nil)))
	p.resolver = r
	p.ready.Store(true)
	return p, r
}

func TestWithApplyTimeUsesTheTypedEvaluationsOriginalResolve(t *testing.T) {
	t.Parallel()
	timestamp := time.Unix(1700000000, 123456789)
	for _, tc := range []struct {
		name string
		eval func(*LocalResolverProvider, context.Context) (any, error)
		want any
	}{
		{"bool", func(p *LocalResolverProvider, ctx context.Context) (any, error) {
			r := p.BooleanEvaluation(ctx, "example.enabled", false, nil)
			return r.Value, r.Error()
		}, true},
		{"string", func(p *LocalResolverProvider, ctx context.Context) (any, error) {
			r := p.StringEvaluation(ctx, "example.title", "", nil)
			return r.Value, r.Error()
		}, "hello"},
		{"int", func(p *LocalResolverProvider, ctx context.Context) (any, error) {
			r := p.IntEvaluation(ctx, "example.count", 0, nil)
			return r.Value, r.Error()
		}, int64(3)},
		{"float", func(p *LocalResolverProvider, ctx context.Context) (any, error) {
			r := p.FloatEvaluation(ctx, "example.ratio", 0, nil)
			return r.Value, r.Error()
		}, 1.5},
		{"object", func(p *LocalResolverProvider, ctx context.Context) (any, error) {
			r := p.ObjectEvaluation(ctx, "example", map[string]any{}, nil)
			return r.Value, r.Error()
		}, map[string]any{"enabled": true, "title": "hello", "count": 3.0, "ratio": 1.5}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			p, r := newApplyTimeProvider(t)
			start := time.Now()
			got, err := tc.eval(p, WithApplyTime(t.Context(), timestamp))
			if err != nil || !reflect.DeepEqual(got, tc.want) {
				t.Fatalf("got (%#v, %v), want %#v", got, err, tc.want)
			}
			if len(r.requests) != 1 || r.requests[0].Apply || r.registrations != 1 || len(r.applications) != 1 {
				t.Fatalf("resolve=%+v registrations=%d apply=%+v", r.requests, r.registrations, r.applications)
			}
			req := r.applications[0]
			if !bytes.Equal(req.ResolveToken, r.response.ResolveToken) || req.ClientSecret != "test-client" ||
				len(req.Flags) != 1 || req.Flags[0].Flag != "flags/example" || !req.Flags[0].ApplyTime.AsTime().Equal(timestamp) {
				t.Fatalf("incorrect deferred application: %v", req)
			}
			if req.SendTime.AsTime().Before(start) || req.SendTime.AsTime().After(time.Now()) {
				t.Fatalf("send time must be current, got %v", req.SendTime)
			}
		})
	}
}

func TestWithApplyTimePreservesExposureSuppressionAndFallbacks(t *testing.T) {
	t.Parallel()
	for _, tc := range []struct {
		name         string
		configure    func(*LocalResolverProvider, *applyTimeResolver)
		evalCtx      openfeature.FlattenedContext
		flag         string
		zeroTime     bool
		wantValue    bool
		wantError    bool
		wantApply    bool
		applications int
	}{
		{name: "skip apply", evalCtx: openfeature.FlattenedContext{"_confidence_skip_apply": true}, wantValue: true},
		{name: "collection disabled", configure: func(p *LocalResolverProvider, _ *applyTimeResolver) { p.disableExposureCollection = true }, wantValue: true},
		{name: "resolver suppresses apply", configure: func(_ *LocalResolverProvider, r *applyTimeResolver) { r.response.ResolvedFlags[0].ShouldApply = false }, wantValue: true},
		{name: "zero clears inherited time", zeroTime: true, wantValue: true, wantApply: true},
		{name: "missing path", flag: "example.absent", wantError: true},
		{name: "type mismatch", flag: "example.title", wantError: true},
		{name: "resolve error", configure: func(_ *LocalResolverProvider, r *applyTimeResolver) { r.resolveErr = errors.New("resolve failed") }, wantError: true},
		{name: "flag missing", configure: func(_ *LocalResolverProvider, r *applyTimeResolver) { r.response.ResolvedFlags = nil }, wantError: true},
		{name: "unexpected flag", configure: func(_ *LocalResolverProvider, r *applyTimeResolver) { r.response.ResolvedFlags[0].Flag = "flags/other" }, wantError: true},
		{name: "fallthrough exposure without variant", configure: func(_ *LocalResolverProvider, r *applyTimeResolver) {
			r.response.ResolvedFlags[0].Variant = ""
			r.response.ResolvedFlags[0].Reason = resolver.ResolveReason_RESOLVE_REASON_NO_SEGMENT_MATCH
		}, applications: 1},
		{name: "no assignment", configure: func(_ *LocalResolverProvider, r *applyTimeResolver) {
			r.response.ResolvedFlags[0].Variant = ""
			r.response.ResolvedFlags[0].ShouldApply = false
		}},
		{name: "apply failure keeps resolved value", configure: func(_ *LocalResolverProvider, r *applyTimeResolver) { r.applyErr = errors.New("apply failed") }, wantValue: true, applications: 1},
	} {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			p, r := newApplyTimeProvider(t)
			if tc.configure != nil {
				tc.configure(p, r)
			}
			ctx := WithApplyTime(t.Context(), time.Unix(1700000000, 0))
			if tc.zeroTime {
				ctx = WithApplyTime(ctx, time.Time{})
			}
			flag := tc.flag
			if flag == "" {
				flag = "example.enabled"
			}
			got := p.BooleanEvaluation(ctx, flag, false, tc.evalCtx)
			if got.Value != tc.wantValue || (got.Error() != nil) != tc.wantError {
				t.Fatalf("unexpected result: %+v", got)
			}
			if len(r.requests) != 1 || r.requests[0].Apply != tc.wantApply || len(r.applications) != tc.applications {
				t.Fatalf("resolve=%+v apply=%+v", r.requests, r.applications)
			}
		})
	}
}
