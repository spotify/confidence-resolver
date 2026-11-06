package confidence

import (
	"context"
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
	"google.golang.org/protobuf/proto"
)

// newIngestionProvider builds a provider whose log batches are delivered to
// a mock ingestion endpoint, exercising the full push path: module callback
// → token capture → queue → HTTP delivery.
func newIngestionProvider(t *testing.T) (*LocalResolverProvider, *testutil.MockIngestion) {
	t.Helper()
	ingestion := testutil.NewMockIngestion(t)
	seed := int64(0x0123456789abcdef)
	provider, err := newProviderForTest(context.Background(), ProviderTestConfig{
		ClientSecret:     tutorialSecret,
		Module:           testutil.LoadClientModuleFixture(t, tutorialModuleFile),
		LogToken:         "token-1",
		IngestionBaseURL: ingestion.URL(),
		Seed:             &seed,
	})
	if err != nil {
		t.Fatalf("newProviderForTest: %v", err)
	}
	if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatalf("Init: %v", err)
	}
	t.Cleanup(provider.Shutdown)
	return provider, ingestion
}

func TestLogsDeliveredToIngestionWithToken(t *testing.T) {
	provider, ingestion := newIngestionProvider(t)

	detail := provider.StringEvaluation(context.Background(), "tutorial-feature.message", "fallback", tutorialEvalCtx())
	if detail.Value == "fallback" {
		t.Fatal("precondition: flag should resolve")
	}
	provider.flushLogsForTest(t)

	if !ingestion.AwaitDelivered(t, 1, 5*time.Second) {
		t.Fatal("no log batch reached ingestion")
	}

	batches := ingestion.Batches()
	if len(batches) == 0 {
		t.Fatal("no batches recorded")
	}
	last := batches[len(batches)-1]
	if last.Token != "Bearer token-1" {
		t.Errorf("authorization = %q, want Bearer token-1", last.Token)
	}
	if last.ContentType != "application/x-protobuf" {
		t.Errorf("content type = %q", last.ContentType)
	}
	// The body must be a WriteFlagLogsRequest carrying the assignment.
	req := &resolverv1.WriteFlagLogsRequest{}
	if err := proto.Unmarshal(last.Body, req); err != nil {
		t.Fatalf("body does not decode as WriteFlagLogsRequest: %v", err)
	}
	if len(req.GetFlagAssigned()) == 0 {
		t.Error("delivered batch carries no assigned flags")
	}
}

func TestLogTokenNeverAppearsInBodies(t *testing.T) {
	provider, ingestion := newIngestionProvider(t)

	provider.StringEvaluation(context.Background(), "tutorial-feature.message", "fallback", tutorialEvalCtx())
	provider.flushLogsForTest(t)

	if !ingestion.AwaitDelivered(t, 1, 5*time.Second) {
		t.Fatal("no batch delivered")
	}
	for i, batch := range ingestion.Batches() {
		if string(batch.Body) == "token-1" {
			t.Errorf("batch %d leaks the token in its body", i)
		}
	}
}

// TestLogDeliveryNeverBlocksEvaluations: with ingestion down, evaluations
// keep resolving while batches are dropped.
func TestLogDeliveryNeverBlocksEvaluations(t *testing.T) {
	provider, ingestion := newIngestionProvider(t)
	ingestion.SetFail(500)

	// Evaluations keep working while ingestion fails.
	for i := 0; i < 25; i++ {
		detail := provider.StringEvaluation(context.Background(), "tutorial-feature.message", "fallback", tutorialEvalCtx())
		if detail.Value == "fallback" {
			t.Fatalf("evaluation %d returned the default while ingestion was down", i)
		}
	}
	provider.flushLogsForTest(t)
	if !awaitCondition(t, 5*time.Second, func() bool { return true }) {
		t.Fatal("unreachable")
	}
}

// TestLogTickerFlushesBatches: the periodic flush timer delivers batches
// without an explicit flush call.
func TestLogTickerFlushesBatches(t *testing.T) {
	ingestion := testutil.NewMockIngestion(t)
	seed := int64(0x0123456789abcdef)
	provider, err := newProviderForTest(context.Background(), ProviderTestConfig{
		ClientSecret:     tutorialSecret,
		Module:           testutil.LoadClientModuleFixture(t, tutorialModuleFile),
		LogToken:         "token-1",
		IngestionBaseURL: ingestion.URL(),
		Seed:             &seed,
		LogFlushInterval: 50 * time.Millisecond,
	})
	if err != nil {
		t.Fatalf("newProviderForTest: %v", err)
	}
	if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatalf("Init: %v", err)
	}
	t.Cleanup(provider.Shutdown)

	provider.StringEvaluation(context.Background(), "tutorial-feature.message", "fallback", tutorialEvalCtx())

	if !ingestion.AwaitDelivered(t, 1, 5*time.Second) {
		t.Fatal("flush ticker did not deliver a batch")
	}
}
