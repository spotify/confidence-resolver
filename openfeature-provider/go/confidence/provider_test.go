package confidence

import (
	"context"
	"strings"
	"testing"

	"github.com/open-feature/go-sdk/openfeature"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
	"google.golang.org/protobuf/proto"
)

const (
	tutorialModuleFile = "tutorial_client_module.wasm"
	e2eModuleFile      = "e2e_client_module.wasm"

	// e2eSecretA is one of the two credentials baked into the e2e fixture
	// module (neither is a live environment secret).
	e2eSecretA = "RxDVTrXvc6op1XxiQ4OaR31dKbJ39aYV"
)

// newTestProvider initializes a provider hosting the tutorial module fixture
// with a log sink that captures batches and tokens.
func newTestProvider(t *testing.T, opts ...func(*ProviderTestConfig)) (*LocalResolverProvider, *capturedLogs) {
	t.Helper()
	return newTestProviderForModule(t, tutorialModuleFile, tutorialSecret, opts...)
}

func newTestProviderForModule(t *testing.T, moduleFile, secret string, opts ...func(*ProviderTestConfig)) (*LocalResolverProvider, *capturedLogs) {
	t.Helper()
	cfg := ProviderTestConfig{
		ClientSecret: secret,
		Module:       testutil.LoadClientModuleFixture(t, moduleFile),
		LogToken:     "test-log-token",
	}
	for _, opt := range opts {
		opt(&cfg)
	}
	logs := newCapturedLogs()
	cfg.LogSink = logs.sink
	if cfg.Seed == nil {
		// Pin the seed to the upstream harness value so variant bucketing
		// is deterministic across runs.
		cfg.Seed = seedPtr(0x0123456789abcdef)
	}
	provider, err := newProviderForTest(context.Background(), cfg)
	if err != nil {
		t.Fatalf("newProviderForTest: %v", err)
	}
	if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatalf("Init: %v", err)
	}
	t.Cleanup(provider.Shutdown)
	return provider, logs
}

// capturedLogs records delivered batches with their tokens.
type capturedLogs struct {
	bodies [][]byte
	tokens []string
}

func newCapturedLogs() *capturedLogs {
	return &capturedLogs{}
}

func (c *capturedLogs) sink(body []byte, token string) {
	c.bodies = append(c.bodies, append([]byte(nil), body...))
	c.tokens = append(c.tokens, token)
}

func tutorialEvalCtx() openfeature.FlattenedContext {
	return openfeature.FlattenedContext{"visitor_id": "tutorial_visitor"}
}

// e2eEvalCtx resolves flags/web-sdk-e2e-flag to its control variant.
func e2eEvalCtx() openfeature.FlattenedContext {
	return openfeature.FlattenedContext{"targeting_key": "test-a", "sticky": false}
}

func TestBooleanEvaluation(t *testing.T) {
	provider, _ := newTestProviderForModule(t, e2eModuleFile, e2eSecretA)

	// The control variant's bool field is false; pass a true default so the
	// resolved value cannot be confused with the echoed default.
	detail := provider.BooleanEvaluation(context.Background(), "web-sdk-e2e-flag.bool", true, e2eEvalCtx())
	if detail.Value != false {
		t.Errorf("value = %v, want false", detail.Value)
	}
	if detail.Variant != "flags/web-sdk-e2e-flag/variants/control" {
		t.Errorf("variant = %q", detail.Variant)
	}
	if detail.Reason != openfeature.TargetingMatchReason {
		t.Errorf("reason = %q", detail.Reason)
	}
	if detail.ResolutionError.Error() != "" && detail.ResolutionError.Error() != ": " {
		t.Errorf("unexpected error: %v", detail.ResolutionError)
	}
}

func TestStringEvaluation(t *testing.T) {
	provider, _ := newTestProvider(t)

	detail := provider.StringEvaluation(context.Background(), "tutorial-feature.message", "fallback", tutorialEvalCtx())
	if detail.Value == "fallback" {
		t.Error("got the default value back")
	}
	if detail.Reason != openfeature.TargetingMatchReason {
		t.Errorf("reason = %q", detail.Reason)
	}
}

func TestFloatAndIntEvaluation(t *testing.T) {
	provider, _ := newTestProviderForModule(t, e2eModuleFile, e2eSecretA)

	// The control variant carries {bool: false, double: 3.5, int: 3,
	// obj: {bool: false, double: 3.6, int: 4, obj-obj: {}, str: "obj
	// control"}, str: "control"}: typed paths exercise number conversion
	// both ways, including nested paths.
	intDetail := provider.IntEvaluation(context.Background(), "web-sdk-e2e-flag.int", 0, e2eEvalCtx())
	if intDetail.Value != 3 {
		t.Errorf("int value = %d, want 3", intDetail.Value)
	}
	floatDetail := provider.FloatEvaluation(context.Background(), "web-sdk-e2e-flag.double", 0, e2eEvalCtx())
	if floatDetail.Value != 3.5 {
		t.Errorf("float value = %v, want 3.5", floatDetail.Value)
	}
	// Nested paths work through the module's evaluate step.
	nested := provider.IntEvaluation(context.Background(), "web-sdk-e2e-flag.obj.int", 0, e2eEvalCtx())
	if nested.Value != 4 {
		t.Errorf("nested int value = %d, want 4", nested.Value)
	}
	nestedFloat := provider.FloatEvaluation(context.Background(), "web-sdk-e2e-flag.obj.double", 0, e2eEvalCtx())
	if nestedFloat.Value != 3.6 {
		t.Errorf("nested float value = %v, want 3.6", nestedFloat.Value)
	}
	// A fractional double does not fit an int request.
	frac := provider.IntEvaluation(context.Background(), "web-sdk-e2e-flag.double", 7, e2eEvalCtx())
	if frac.Value != 7 || errorCodeOf(frac.ResolutionError) != openfeature.TypeMismatchCode {
		t.Errorf("fractional-to-int = %d err %v", frac.Value, frac.ResolutionError)
	}
}

func TestObjectEvaluation(t *testing.T) {
	provider, _ := newTestProvider(t)

	detail := provider.ObjectEvaluation(context.Background(), "tutorial-feature", map[string]interface{}{}, tutorialEvalCtx())
	obj, ok := detail.Value.(map[string]interface{})
	if !ok {
		t.Fatalf("value type %T, want map", detail.Value)
	}
	if obj["title"] != "Welcome to Confidence!" {
		t.Errorf("title = %v", obj["title"])
	}
	if detail.Reason != openfeature.TargetingMatchReason {
		t.Errorf("reason = %q", detail.Reason)
	}
}

func TestTypeMismatchReturnsDefault(t *testing.T) {
	provider, _ := newTestProvider(t)

	// The message field is a string; asking for a bool is a mismatch.
	detail := provider.BooleanEvaluation(context.Background(), "tutorial-feature.message", true, tutorialEvalCtx())
	if detail.Value != true {
		t.Errorf("default not echoed: %v", detail.Value)
	}
	if errorCodeOf(detail.ResolutionError) != openfeature.TypeMismatchCode {
		t.Errorf("error code = %q", errorCodeOf(detail.ResolutionError))
	}
}

func TestFlagNotFoundReturnsDefault(t *testing.T) {
	provider, _ := newTestProvider(t)

	detail := provider.StringEvaluation(context.Background(), "no-such-flag", "fallback", tutorialEvalCtx())
	if detail.Value != "fallback" {
		t.Errorf("value = %q, want fallback", detail.Value)
	}
	if errorCodeOf(detail.ResolutionError) != openfeature.FlagNotFoundCode {
		t.Errorf("error code = %q", errorCodeOf(detail.ResolutionError))
	}
}

func TestEvaluationBeforeInitReturnsNotReady(t *testing.T) {
	cfg := ProviderTestConfig{
		ClientSecret: tutorialSecret,
		Module:       testutil.LoadClientModuleFixture(t, tutorialModuleFile),
		LogToken:     "tok",
	}
	provider, err := newProviderForTest(context.Background(), cfg)
	if err != nil {
		t.Fatalf("newProviderForTest: %v", err)
	}
	// Deliberately no Init.

	detail := provider.BooleanEvaluation(context.Background(), "tutorial-feature.message", false, tutorialEvalCtx())
	if detail.Value != false {
		t.Errorf("value = %v, want default", detail.Value)
	}
	if errorCodeOf(detail.ResolutionError) != openfeature.ProviderNotReadyCode {
		t.Errorf("error code = %q", errorCodeOf(detail.ResolutionError))
	}
}

func TestInvalidContextReturnsInvalidContextError(t *testing.T) {
	provider, _ := newTestProvider(t)

	detail := provider.BooleanEvaluation(context.Background(), "tutorial-feature.message", false, openfeature.FlattenedContext{
		"bad": complex(1, 2), // not representable in the evaluation context
	})
	if detail.Value != false {
		t.Errorf("value = %v, want default", detail.Value)
	}
	if errorCodeOf(detail.ResolutionError) != openfeature.InvalidContextCode {
		t.Errorf("error code = %q", errorCodeOf(detail.ResolutionError))
	}
}

func TestSkipApplyContextKey(t *testing.T) {
	provider, logs := newTestProvider(t)

	ctx := openfeature.FlattenedContext{
		"visitor_id":             "tutorial_visitor",
		"_confidence_skip_apply": true,
	}
	detail := provider.StringEvaluation(context.Background(), "tutorial-feature.message", "", ctx)
	if detail.Value == "" {
		t.Fatal("got default despite a resolving flag")
	}
	// With skip-apply the resolve must not record an assignment. Resolve
	// analytics may still be delivered; assert on the assignment events.
	provider.flushLogsForTest(t)
	if assigned := countAssignedEvents(t, logs.bodies); assigned != 0 {
		t.Errorf("skip-apply still logged %d assigned flags", assigned)
	}
}

func TestDisableExposureCollection(t *testing.T) {
	provider, logs := newTestProvider(t, func(cfg *ProviderTestConfig) {
		cfg.DisableExposureCollection = true
	})

	detail := provider.StringEvaluation(context.Background(), "tutorial-feature.message", "", tutorialEvalCtx())
	if detail.Value == "" {
		t.Fatal("got default despite a resolving flag")
	}
	provider.flushLogsForTest(t)
	if assigned := countAssignedEvents(t, logs.bodies); assigned != 0 {
		t.Errorf("exposure collection still logged %d assigned flags", assigned)
	}
}

func TestResolvePublicAPI(t *testing.T) {
	provider, _ := newTestProvider(t)

	response, err := provider.Resolve(context.Background(), tutorialEvalCtx(), []string{"tutorial-feature"}, true)
	if err != nil {
		t.Fatalf("Resolve: %v", err)
	}
	if len(response.GetResolvedFlags()) != 1 {
		t.Fatalf("resolved %d flags", len(response.GetResolvedFlags()))
	}
	if response.GetResolvedFlags()[0].GetFlag() != "flags/tutorial-feature" {
		t.Errorf("flag = %q", response.GetResolvedFlags()[0].GetFlag())
	}
}

func TestResolveDeferredApplyFlow(t *testing.T) {
	provider, _ := newTestProvider(t)

	response, err := provider.Resolve(context.Background(), tutorialEvalCtx(), []string{"tutorial-feature"}, false)
	if err != nil {
		t.Fatalf("Resolve: %v", err)
	}
	token := response.GetResolveToken()
	if len(token) == 0 {
		t.Fatal("apply=false resolve returned no token")
	}

	if err := provider.ApplyFlags(&resolver.ApplyFlagsRequest{
		ResolveToken: token,
		Flags:        []*resolver.AppliedFlag{{Flag: "flags/tutorial-feature"}},
	}); err != nil {
		t.Fatalf("ApplyFlags: %v", err)
	}
}

func TestTargetingKeyIsNormalized(t *testing.T) {
	provider, _ := newTestProviderForModule(t, e2eModuleFile, e2eSecretA)

	// "targetingKey" is the OpenFeature name; the resolver expects
	// "targeting_key". The e2e flag resolves its control variant for
	// targeting_key "test-a".
	detail := provider.StringEvaluation(context.Background(), "web-sdk-e2e-flag.str", "fallback", openfeature.FlattenedContext{
		"targetingKey": "test-a",
		"sticky":       false,
	})
	if detail.Value != "control" {
		t.Errorf("value = %q, want the control variant's top-level string (targetingKey not normalized)", detail.Value)
	}
}

// countAssignedEvents counts assigned-flag events across delivered batches.
func countAssignedEvents(t *testing.T, bodies [][]byte) int {
	t.Helper()
	total := 0
	for _, body := range bodies {
		req := &resolverv1.WriteFlagLogsRequest{}
		if err := proto.Unmarshal(body, req); err != nil {
			t.Fatalf("log batch does not decode: %v", err)
		}
		total += len(req.GetFlagAssigned())
	}
	return total
}

// seedPtr boxes a seed for ProviderTestConfig.
func seedPtr(v int64) *int64 { return &v }

// flushLogsForTest drives one flush so delivered batches are observable
// without waiting for the flush ticker.
func (p *LocalResolverProvider) flushLogsForTest(t *testing.T) {
	t.Helper()
	p.hostMu.RLock()
	host := p.host
	p.hostMu.RUnlock()
	if host != nil {
		if err := host.FlushLogs(); err != nil {
			t.Fatalf("FlushLogs: %v", err)
		}
	}
}

// errorCodeOf extracts the error code from a ResolutionError's text form
// ("CODE: message").
func errorCodeOf(err openfeature.ResolutionError) openfeature.ErrorCode {
	text := err.Error()
	if idx := strings.Index(text, ":"); idx >= 0 {
		return openfeature.ErrorCode(text[:idx])
	}
	return openfeature.ErrorCode(text)
}

func TestObjectEvaluationStructDefault(t *testing.T) {
	provider, _ := newTestProviderForModule(t, e2eModuleFile, e2eSecretA)

	type nested struct {
		Str string `json:"str"`
	}
	type e2eValue struct {
		Bool   bool   `json:"bool"`
		Int    int    `json:"int"`
		Nested nested `json:"obj"`
	}

	// All fields present in the resolved control variant: the result is a
	// fully populated struct of the default's type.
	defaultValue := e2eValue{Bool: true, Int: 99, Nested: nested{Str: "default"}}
	detail := provider.ObjectEvaluation(context.Background(), "web-sdk-e2e-flag", defaultValue, e2eEvalCtx())
	obj, ok := detail.Value.(e2eValue)
	if !ok {
		t.Fatalf("value type %T, want e2eValue", detail.Value)
	}
	if obj.Bool != false || obj.Int != 3 || obj.Nested.Str != "obj control" {
		t.Errorf("struct not populated from resolved value: %+v", obj)
	}
}

func TestObjectEvaluationMapMerge(t *testing.T) {
	provider, _ := newTestProviderForModule(t, e2eModuleFile, e2eSecretA)

	defaultValue := map[string]interface{}{
		"str":    "fallback",
		"absent": "kept-from-default",
	}
	detail := provider.ObjectEvaluation(context.Background(), "web-sdk-e2e-flag", defaultValue, e2eEvalCtx())
	obj, ok := detail.Value.(map[string]interface{})
	if !ok {
		t.Fatalf("value type %T, want map", detail.Value)
	}
	if obj["str"] != "control" {
		t.Errorf("str = %v, want resolved value", obj["str"])
	}
	if obj["absent"] != "kept-from-default" {
		t.Errorf("absent = %v, want default preserved", obj["absent"])
	}
}
