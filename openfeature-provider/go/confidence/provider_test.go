package confidence

import (
	"context"
	"io"
	"log/slog"
	"strings"
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/s2wpb/resolverpb"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
)

func newTestProvider(t *testing.T, mod func(*ProviderConfig)) (*LocalResolverProvider, *testutil.Backend) {
	t.Helper()
	b := testutil.NewBackend(t)
	b.Publish(testutil.Module(t, "basic.wasm"), "log-token", `"v1"`)
	cfg := ProviderConfig{
		ClientSecret: testutil.Secret, EncryptionKey: b.HexKey(), Instances: 2,
		Logger:          slog.New(slog.NewTextHandler(io.Discard, nil)),
		ResolverBaseURL: b.Resolver.URL,
		cdnBaseURL:      b.CDN.URL, ingestBaseURL: b.Ingest.URL, retryInterval: 5 * time.Millisecond,
	}
	if mod != nil {
		mod(&cfg)
	}
	p, err := NewProvider(context.Background(), cfg)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(p.Shutdown)
	return p, b
}

func started(t *testing.T, mod func(*ProviderConfig)) (*LocalResolverProvider, *testutil.Backend) {
	p, b := newTestProvider(t, mod)
	if err := p.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatal(err)
	}
	return p, b
}

var user = openfeature.FlattenedContext{openfeature.TargetingKey: "user-1"}

type plainFlag struct {
	Enabled bool    `json:"enabled"`
	Message string  `json:"message"`
	Count   int     `json:"count"`
	Ratio   float64 `json:"ratio"`
	Nested  struct {
		Label string
	}
}

func TestEvaluations(t *testing.T) {
	p, _ := started(t, nil)
	ctx := context.Background()

	if d := p.BooleanEvaluation(ctx, "plain.enabled", false, user); !d.Value || d.Variant != "flags/plain/variants/on" || d.Reason != openfeature.TargetingMatchReason {
		t.Errorf("bool: %+v", d)
	}
	if d := p.StringEvaluation(ctx, "plain.message", "", user); d.Value != "hello" {
		t.Errorf("string: %+v", d)
	}
	if d := p.IntEvaluation(ctx, "plain.count", 0, user); d.Value != 42 {
		t.Errorf("int: %+v", d)
	}
	if d := p.FloatEvaluation(ctx, "plain.ratio", 0, user); d.Value != 0.5 {
		t.Errorf("float: %+v", d)
	}
	if d := p.StringEvaluation(ctx, "plain.nested.label", "", user); d.Value != "inner" {
		t.Errorf("nested path: %+v", d)
	}

	d := p.ObjectEvaluation(ctx, "plain", plainFlag{}, user)
	got, ok := d.Value.(plainFlag)
	if !ok || !got.Enabled || got.Message != "hello" || got.Count != 42 || got.Ratio != 0.5 || got.Nested.Label != "inner" {
		t.Errorf("struct: %+v (%v)", d.Value, d.ResolutionError)
	}
	if d := p.ObjectEvaluation(ctx, "plain", &plainFlag{}, user); d.Value.(*plainFlag).Message != "hello" {
		t.Errorf("pointer: %+v", d)
	}
	if d := p.ObjectEvaluation(ctx, "plain.nested", map[string]string{}, user); d.Value.(map[string]string)["label"] != "inner" {
		t.Errorf("map: %+v (%v)", d.Value, d.ResolutionError)
	}
	if d := p.ObjectEvaluation(ctx, "plain", nil, user); d.Value.(map[string]any)["message"] != "hello" {
		t.Errorf("nil default: %+v", d)
	}
}

func TestEvaluationErrors(t *testing.T) {
	p, _ := started(t, nil)
	ctx := context.Background()
	for name, tc := range map[string]struct {
		d    openfeature.ProviderResolutionDetail
		code openfeature.ErrorCode
	}{
		"unknown flag":      {p.BooleanEvaluation(ctx, "nope", true, user).ProviderResolutionDetail, openfeature.FlagNotFoundCode},
		"type mismatch":     {p.BooleanEvaluation(ctx, "plain.message", true, user).ProviderResolutionDetail, openfeature.TypeMismatchCode},
		"double as int":     {p.ObjectEvaluation(ctx, "plain", struct{ Ratio int }{}, user).ProviderResolutionDetail, openfeature.TypeMismatchCode},
		"map element type":  {p.ObjectEvaluation(ctx, "plain.nested", map[string]int{}, user).ProviderResolutionDetail, openfeature.TypeMismatchCode},
		"no store (sticky)": {p.FloatEvaluation(ctx, "sticky.value", -1, user).ProviderResolutionDetail, openfeature.GeneralCode},
	} {
		if got := tc.d.ResolutionDetail().ErrorCode; got != tc.code || tc.d.Reason != openfeature.ErrorReason {
			t.Errorf("%s: code=%v reason=%v (%v), want %v", name, got, tc.d.Reason, tc.d.ResolutionError, tc.code)
		}
	}
	// The default is returned on error.
	if d := p.BooleanEvaluation(ctx, "plain.message", true, user); !d.Value {
		t.Error("default not returned on type mismatch")
	}
}

func TestNotReadyBeforeInit(t *testing.T) {
	p, _ := newTestProvider(t, nil)
	d := p.BooleanEvaluation(context.Background(), "plain.enabled", false, user)
	if d.ResolutionDetail().ErrorCode != openfeature.ProviderNotReadyCode {
		t.Errorf("before Init: %+v", d)
	}
}

// stickyStore answers variant reads with the sticky variant.
type stickyStore struct{ writes []WriteOp }

func (s *stickyStore) Read(_ context.Context, ops []ReadOp) ([]ReadResult, error) {
	var out []ReadResult
	sticky := "flags/sticky/variants/sticky"
	for _, op := range ops {
		if v, ok := op.(*ReadOpVariant); ok {
			out = append(out, v.ToResult(&sticky))
		}
	}
	return out, nil
}

func (s *stickyStore) Write(_ context.Context, ops []WriteOp) error {
	s.writes = append(s.writes, ops...)
	return nil
}

func TestMaterializationStore(t *testing.T) {
	p, _ := started(t, func(c *ProviderConfig) { c.MaterializationStore = &stickyStore{} })
	d := p.FloatEvaluation(context.Background(), "sticky.value", -1, user)
	if d.Value != 99.99 || d.Variant != "flags/sticky/variants/sticky" {
		t.Errorf("sticky: %+v (%v)", d, d.ResolutionError)
	}
}

func TestDeferredApply(t *testing.T) {
	p, b := started(t, nil)
	resp, err := p.Resolve(context.Background(), user, []string{"plain"}, false)
	if err != nil {
		t.Fatal(err)
	}
	if len(resp.GetResolveToken()) == 0 || resp.GetResolvedFlags()[0].GetVariant() != "flags/plain/variants/on" {
		t.Fatalf("response: %v", resp)
	}
	err = p.ApplyFlags(&resolverpb.ApplyFlagsRequest{
		ResolveToken: resp.GetResolveToken(),
		Flags:        []*resolverpb.AppliedFlag{{Flag: "flags/plain"}},
	})
	if err != nil {
		t.Fatal(err)
	}
	p.Shutdown()
	if n := assigned(b); n != 1 {
		t.Errorf("%d exposures logged after a deferred apply, want 1", n)
	}
}

func assigned(b *testutil.Backend) int {
	n := 0
	for _, req := range b.Logs() {
		for _, fa := range req.GetFlagAssigned() {
			n += len(fa.GetFlags())
		}
	}
	return n
}

func TestExposureModes(t *testing.T) {
	for mode, want := range map[ExposureMode]int{ExposureDeduplicated: 1, ExposureAll: 3, ExposureNone: 0} {
		p, b := started(t, func(c *ProviderConfig) { c.ExposureMode = mode })
		for range 3 {
			p.BooleanEvaluation(context.Background(), "plain.enabled", false, user)
		}
		p.Shutdown()
		if n := assigned(b); n != want {
			t.Errorf("mode %v: %d exposures for 3 identical evaluations, want %d", mode, n, want)
		}
		for _, batch := range b.Batches() {
			if batch.Token != "log-token" {
				t.Errorf("batch sent with token %q", batch.Token)
			}
		}
	}
}

// _confidence_skip_apply set to true skips exposure for that evaluation only; any other value is
// ignored. The key never reaches the module, so it isn't in the logged context schema either.
func TestSkipApply(t *testing.T) {
	p, b := started(t, func(c *ProviderConfig) { c.ExposureMode = ExposureAll })
	for _, skip := range []any{true, "true", false} {
		evalCtx := openfeature.FlattenedContext{openfeature.TargetingKey: "user-1", skipApplyKey: skip}
		if d := p.BooleanEvaluation(context.Background(), "plain.enabled", false, evalCtx); d.Error() != nil {
			t.Fatalf("skip %v: %v", skip, d.Error())
		}
	}
	p.Shutdown()
	if n := assigned(b); n != 2 {
		t.Errorf("%d exposures, want 2 (for the evaluations not skipped)", n)
	}
	resolves := 0
	for _, req := range b.Logs() {
		for _, info := range req.GetClientResolveInfo() {
			for _, s := range info.GetSchema() {
				resolves++
				if _, ok := s.GetSchema()[skipApplyKey]; ok {
					t.Errorf("%s reached the module", skipApplyKey)
				}
			}
		}
	}
	if resolves == 0 {
		t.Error("no resolve info logged")
	}
}

func TestMetrics(t *testing.T) {
	p, _ := started(t, nil)
	p.BooleanEvaluation(context.Background(), "plain.enabled", false, user)
	if m := p.GetPrometheusMetrics(SnapshotConfig{}); !strings.Contains(m, "resolve") {
		t.Errorf("prometheus: %q", m)
	}
	if m := p.GetPrometheusMetrics(SnapshotConfig{OpenMetrics: true}); !strings.HasSuffix(strings.TrimSpace(m), "# EOF") {
		t.Errorf("openmetrics doesn't end with # EOF: %q", m)
	}
}

func TestConfigValidation(t *testing.T) {
	good := strings.Repeat("ab", 32)
	for name, cfg := range map[string]ProviderConfig{
		"no secret":   {EncryptionKey: good},
		"no key":      {ClientSecret: "s"},
		"short key":   {ClientSecret: "s", EncryptionKey: good[2:]},
		"non-hex key": {ClientSecret: "s", EncryptionKey: strings.Repeat("zz", 32)},
		"both stores": {ClientSecret: "s", EncryptionKey: good, MaterializationStore: &stickyStore{}, UseRemoteMaterializationStore: true},
	} {
		if _, err := NewProvider(context.Background(), cfg); err == nil {
			t.Errorf("%s: accepted", name)
		} else if strings.Contains(err.Error(), good) {
			t.Errorf("%s: error leaks the key", name)
		}
	}
}

func TestOpenFeature(t *testing.T) {
	p, b := newTestProvider(t, nil)
	if err := openfeature.SetNamedProviderAndWait(t.Name(), p); err != nil {
		t.Fatal(err)
	}
	client := openfeature.NewClient(t.Name())
	v, err := client.BooleanValue(context.Background(), "plain.enabled", false, openfeature.NewEvaluationContext("user-1", nil))
	if err != nil || !v {
		t.Fatalf("value=%v err=%v", v, err)
	}
	openfeature.Shutdown()
	if assigned(b) != 1 {
		t.Errorf("shutdown didn't flush the exposure: %d", assigned(b))
	}
}

func TestStartupNotPublishedThenReady(t *testing.T) {
	p, b := newTestProvider(t, func(c *ProviderConfig) {
		c.InitializationTimeout = 30 * time.Millisecond
		c.StatePollInterval = 10 * time.Millisecond
	})
	b.SetStatus(404)
	if err := p.Init(openfeature.EvaluationContext{}); err == nil {
		t.Fatal("Init succeeded without a module")
	}
	b.Publish(testutil.Module(t, "basic.wasm"), "log-token", `"v2"`)
	select {
	case ev := <-p.EventChannel():
		if ev.EventType != openfeature.ProviderReady {
			t.Fatalf("event %v", ev.EventType)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("no ProviderReady")
	}
	if d := p.BooleanEvaluation(context.Background(), "plain.enabled", false, user); !d.Value {
		t.Errorf("after recovery: %+v", d)
	}
}
