package confidence

import (
	"context"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
)

// newCDNProvider builds a full provider against a mock CDN serving the
// tutorial module wrapped with a freshly generated encryption key: the whole
// init flow (fetch → decrypt → wrapper parse → validate → instantiate) runs.
func newCDNProvider(t *testing.T, opts ...func(*ProviderConfig)) (*LocalResolverProvider, *testutil.MockModuleCDN, string) {
	t.Helper()
	cdn := testutil.NewMockModuleCDN(t)
	key := testutil.GenerateEncryptionKey()
	module := testutil.LoadClientModuleFixture(t, tutorialModuleFile)
	cdn.SetArtifact(tutorialSecret, module, "token-1", key)

	cfg := ProviderConfig{
		ClientSecret:       tutorialSecret,
		EncryptionKey:      key,
		ModuleBaseURL:      cdn.URL(),
		IngestionBaseURL:   "http://ingestion.invalid",
		Logger:             testLogger(t),
		ModulePollInterval: 50 * time.Millisecond,
	}
	for _, opt := range opts {
		opt(&cfg)
	}
	provider, err := NewProvider(context.Background(), cfg)
	if err != nil {
		t.Fatalf("NewProvider: %v", err)
	}
	return provider, cdn, key
}

func TestInitFullFlowLoadsModule(t *testing.T) {
	provider, _, _ := newCDNProvider(t)
	t.Cleanup(provider.Shutdown)

	if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatalf("Init: %v", err)
	}
	if !provider.ready.Load() {
		t.Fatal("provider not ready after successful init")
	}
	detail := provider.StringEvaluation(context.Background(), "tutorial-feature.message", "fallback", tutorialEvalCtx())
	if detail.Value == "fallback" {
		t.Error("evaluation returned the default after a successful init")
	}
}

func TestInitTimesOutWhenNoModulePublished(t *testing.T) {
	// No artifact installed: 404s. Init must fail after its retry budget but
	// keep polling; once a module appears the provider recovers to READY.
	cdn := testutil.NewMockModuleCDN(t)
	key := testutil.GenerateEncryptionKey()
	cfg := ProviderConfig{
		ClientSecret:          tutorialSecret,
		EncryptionKey:         key,
		ModuleBaseURL:         cdn.URL(),
		IngestionBaseURL:      "http://ingestion.invalid",
		Logger:                testLogger(t),
		ModulePollInterval:    20 * time.Millisecond,
		InitializationTimeout: 200 * time.Millisecond,
	}
	provider, err := NewProvider(context.Background(), cfg)
	if err != nil {
		t.Fatalf("NewProvider: %v", err)
	}
	t.Cleanup(provider.Shutdown)

	err = provider.Init(openfeature.EvaluationContext{})
	if err == nil {
		t.Fatal("expected init to time out with no module published")
	}

	// Evaluations report PROVIDER_NOT_READY with the default.
	detail := provider.StringEvaluation(context.Background(), "tutorial-feature.message", "fallback", tutorialEvalCtx())
	if detail.Value != "fallback" {
		t.Error("default not served before the module loads")
	}

	// Publish the module; the background retry picks it up.
	cdn.SetArtifact(tutorialSecret, testutil.LoadClientModuleFixture(t, tutorialModuleFile), "token-1", key)
	if !awaitCondition(t, 5*time.Second, func() bool { return provider.ready.Load() }) {
		t.Fatal("provider did not recover after the module was published")
	}
	detail = provider.StringEvaluation(context.Background(), "tutorial-feature.message", "fallback", tutorialEvalCtx())
	if detail.Value == "fallback" {
		t.Error("evaluation still returned the default after recovery")
	}
}

// TestLifecycleRecoveryEvent: a provider whose init timed out (no module
// published yet) reports its recovery on the event channel when the
// background retry finally loads a module. A successful Init itself does
// not emit a redundant Ready event: the OpenFeature SDK emits one when Init
// returns nil.
func TestLifecycleRecoveryEvent(t *testing.T) {
	cdn := testutil.NewMockModuleCDN(t)
	key := testutil.GenerateEncryptionKey()
	cfg := ProviderConfig{
		ClientSecret:          tutorialSecret,
		EncryptionKey:         key,
		ModuleBaseURL:         cdn.URL(),
		IngestionBaseURL:      "http://ingestion.invalid",
		Logger:                testLogger(t),
		ModulePollInterval:    20 * time.Millisecond,
		InitializationTimeout: 150 * time.Millisecond,
	}
	provider, err := NewProvider(context.Background(), cfg)
	if err != nil {
		t.Fatalf("NewProvider: %v", err)
	}
	t.Cleanup(provider.Shutdown)

	if err := provider.Init(openfeature.EvaluationContext{}); err == nil {
		t.Fatal("expected init to time out")
	}

	// Drain any events emitted before the module appeared.
	for {
		select {
		case <-provider.EventChannel():
			continue
		default:
		}
		break
	}

	cdn.SetArtifact(tutorialSecret, testutil.LoadClientModuleFixture(t, tutorialModuleFile), "token-1", key)

	select {
	case event := <-provider.EventChannel():
		if event.EventType != openfeature.ProviderReady {
			t.Errorf("event type = %s, want ProviderReady", event.EventType)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("no ProviderReady recovery event")
	}
}

// TestInstancePoolSizeConfigured runs the provider with explicit pool caps:
// the knob must reach the host without changing evaluation results.
func TestInstancePoolSizeConfigured(t *testing.T) {
	for _, size := range []int{1, 4} {
		t.Run(fmt.Sprintf("size-%d", size), func(t *testing.T) {
			provider, _, _ := newCDNProvider(t, func(cfg *ProviderConfig) {
				cfg.InstancePoolSize = size
			})
			t.Cleanup(provider.Shutdown)

			if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
				t.Fatalf("Init: %v", err)
			}
			detail := provider.StringEvaluation(context.Background(), "tutorial-feature.message", "fallback", tutorialEvalCtx())
			if !strings.HasPrefix(detail.Value, "We are very excited") {
				t.Fatalf("evaluation with pool size %d returned %q", size, detail.Value)
			}
		})
	}
}
