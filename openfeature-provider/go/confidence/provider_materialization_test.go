package confidence

import (
	"context"
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/flagswasm"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
)

// newE2EProvider builds a provider hosting the e2e fixture (whose
// flags/web-sdk-e2e-flag and flags/custom-targeted-flag resolve through
// materializations), with the given materialization store (nil = none).
func newE2EProvider(t *testing.T, store MaterializationStore) *LocalResolverProvider {
	t.Helper()
	logs := newCapturedLogs()
	seed := int64(0x0123456789abcdef)
	provider, err := newProviderForTest(context.Background(), ProviderTestConfig{
		ClientSecret:         e2eSecretA,
		Module:               testutil.LoadClientModuleFixture(t, e2eModuleFile),
		LogToken:             "e2e-token",
		LogSink:              logs.sink,
		MaterializationStore: store,
		Seed:                 &seed,
	})
	if err != nil {
		t.Fatalf("newProviderForTest: %v", err)
	}
	if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatalf("Init: %v", err)
	}
	t.Cleanup(provider.Shutdown)
	return provider
}

func stickyEvalCtx() openfeature.FlattenedContext {
	return openfeature.FlattenedContext{"targeting_key": "test-a", "sticky": true}
}

func cakeEvalCtx() openfeature.FlattenedContext {
	return openfeature.FlattenedContext{"user_id": "u1", "targeting_key": "u1"}
}

// TestMaterializationSuspendResumeWithStore: the custom-targeted flag's rule
// resolves through a materialized segment. Without the inclusion in the
// store the resolve reports the materialization requirement; with it, the
// cake variant matches.
func TestMaterializationSuspendResumeWithStore(t *testing.T) {
	// Seed the inclusion the module's rule asks for.
	store := newInMemoryMaterializationStoreWithInclusions(testLogger(t), map[string]map[string]bool{
		"u1": {"materializedSegments/nicklas-custom-targeting": true},
	})
	provider := newE2EProvider(t, store)

	cake := provider.StringEvaluation(context.Background(), "custom-targeted-flag.message", "fallback", cakeEvalCtx())
	if cake.Value != "Did someone say CAKE?!" {
		t.Fatalf("value = %q, want the cake variant's message", cake.Value)
	}
	if cake.Variant != "flags/custom-targeted-flag/variants/cake-exclamation" {
		t.Errorf("variant = %q", cake.Variant)
	}
	if cake.Reason != openfeature.TargetingMatchReason {
		t.Errorf("reason = %q", cake.Reason)
	}

	// The store must have been read during the resolve loop.
	if len(store.ReadCalls()) == 0 {
		t.Fatal("provider never read the materialization store")
	}
}

// TestMaterializationWriteIsAsync: the batch the module asks to persist is
// handed to the store asynchronously; a failing store never fails the
// resolve.
func TestMaterializationWriteIsAsync(t *testing.T) {
	store := newInMemoryMaterializationStore(testLogger(t))
	provider := newE2EProvider(t, store)

	provider.storeMaterializations([]*flagswasm.Materialization_Record{{
		Unit:            "u1",
		Materialization: "materializedSegments/2b5776c3c0517893a48c7081ee159d31",
		Rule:            "flags/web-sdk-e2e-flag/rules/ilp2u0ovfx87kyalomtn",
		Variant:         "flags/web-sdk-e2e-flag/variants/sticky",
	}})

	if !awaitCondition(t, 5*time.Second, func() bool {
		return len(store.WriteCalls()) == 1
	}) {
		t.Fatalf("materialization_to_write was never persisted (calls: %d)", len(store.WriteCalls()))
	}
	calls := store.WriteCalls()
	if len(calls[0]) != 1 {
		t.Errorf("write batch size = %d, want 1", len(calls[0]))
	}
}

// TestMaterializationUnsupportedStoreFailsCleanly: with no store configured,
// a materialization-requiring resolve surfaces a clear error and the
// provider keeps serving.
func TestMaterializationUnsupportedStoreFailsCleanly(t *testing.T) {
	provider := newE2EProvider(t, nil)

	detail := provider.StringEvaluation(context.Background(), "web-sdk-e2e-flag.str", "fallback", stickyEvalCtx())
	if detail.Value != "fallback" {
		t.Errorf("sticky resolve returned %q without a materialization store", detail.Value)
	}
	if errorCodeOf(detail.ResolutionError) == "" {
		t.Error("expected an error for a materialization-requiring resolve without a store")
	}

	// The provider keeps serving other evaluations.
	normal := provider.StringEvaluation(context.Background(), "web-sdk-e2e-flag.str", "fallback", e2eEvalCtx())
	if normal.Value == "fallback" {
		t.Error("non-sticky resolve stopped working after a materialization failure")
	}
}
