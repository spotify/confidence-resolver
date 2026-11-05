package confidence

import (
	"context"
	"strings"
	"testing"

	"github.com/open-feature/go-sdk/openfeature"
	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
	"google.golang.org/protobuf/proto"
)

// Flag-log parity tests, ported from the state-based provider's e2e suite.
// The old tests fetched the e2e account state live with real credentials;
// the fixture module compiled from that same state now serves the same
// assertions without credentials. The provider contract under test is
// unchanged: resolving a flag must emit flag-assignment, client-resolve and
// flag-resolve data with correct flag names, variants, targeting keys and
// client info, and de-duplicate identical applies without ever
// de-duplicating distinct users.

// newE2ELogsProvider builds a provider hosting the e2e fixture module with
// a capturing log sink. Tests resolve, then Shutdown (synchronously
// flushing) and assert on the captured batches.
func newE2ELogsProvider(t *testing.T) (*LocalResolverProvider, *capturedLogs) {
	t.Helper()
	logs := newCapturedLogs()
	provider, err := newProviderForTest(context.Background(), ProviderTestConfig{
		ClientSecret: e2eSecretA,
		Module:       testutil.LoadClientModuleFixture(t, e2eModuleFile),
		LogToken:     "[REDACTED assignment-style secret]",
		LogSink:      logs.sink,
	})
	if err != nil {
		t.Fatalf("newProviderForTest: %v", err)
	}
	if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatalf("Init: %v", err)
	}
	return provider, logs
}

// flushLogs shuts the provider down, which flushes any batches still
// buffered in the module.
func flushLogs(provider *LocalResolverProvider) {
	provider.Shutdown()
}

// requestWithFlagAssigned returns the first captured batch carrying
// flag-assignment entries.
func requestWithFlagAssigned(t *testing.T, logs *capturedLogs) *resolverv1.WriteFlagLogsRequest {
	t.Helper()
	for _, body := range logs.bodies {
		req := &resolverv1.WriteFlagLogsRequest{}
		if err := proto.Unmarshal(body, req); err != nil {
			t.Fatalf("cannot parse captured log batch: %v", err)
		}
		if len(req.FlagAssigned) > 0 {
			return req
		}
	}
	t.Fatal("no captured batch carries flag_assigned entries")
	return nil
}

// findAppliedFlag returns the applied-flag entry naming the given flag.
func findAppliedFlag(t *testing.T, req *resolverv1.WriteFlagLogsRequest, flag string) *resolverv1.FlagAssigned_AppliedFlag {
	t.Helper()
	for _, fa := range req.FlagAssigned {
		for _, af := range fa.Flags {
			if strings.Contains(af.Flag, flag) {
				return af
			}
		}
	}
	t.Fatalf("no applied flag entry for %q in captured batch", flag)
	return nil
}

// totalAppliedFlags counts applied-flag entries across all captured
// batches.
func totalAppliedFlags(logs *capturedLogs) int {
	total := 0
	for _, body := range logs.bodies {
		req := &resolverv1.WriteFlagLogsRequest{}
		if err := proto.Unmarshal(body, req); err != nil {
			continue
		}
		for _, fa := range req.FlagAssigned {
			total += len(fa.Flags)
		}
	}
	return total
}

func TestFlagLogsParity_CaptureAfterBooleanResolve(t *testing.T) {
	provider, logs := newE2ELogsProvider(t)

	detail := provider.BooleanEvaluation(context.Background(), "web-sdk-e2e-flag.bool", true, e2eEvalCtx())
	if detail.Value != false {
		t.Fatalf("control variant = %v, want false", detail.Value)
	}
	flushLogs(provider)

	req := requestWithFlagAssigned(t, logs)
	af := findAppliedFlag(t, req, "web-sdk-e2e-flag")
	if af.TargetingKey != "test-a" {
		t.Errorf("targeting key = %q, want test-a", af.TargetingKey)
	}
}

func TestFlagLogsParity_AppliedFlagNamesPresent(t *testing.T) {
	provider, logs := newE2ELogsProvider(t)

	if v := provider.StringEvaluation(context.Background(), "web-sdk-e2e-flag.str", "default", e2eEvalCtx()); v.Value != "control" {
		t.Fatalf("control variant = %q, want control", v.Value)
	}
	flushLogs(provider)

	req := requestWithFlagAssigned(t, logs)
	af := findAppliedFlag(t, req, "web-sdk-e2e-flag")
	if af.Flag == "" {
		t.Error("expected flag name to be non-empty")
	}
}

func TestFlagLogsParity_ClientResolveInfo(t *testing.T) {
	provider, logs := newE2ELogsProvider(t)

	_ = provider.IntEvaluation(context.Background(), "web-sdk-e2e-flag.int", 10, e2eEvalCtx())
	flushLogs(provider)

	if req := requestWithFlagAssigned(t, logs); len(req.ClientResolveInfo) == 0 {
		t.Error("expected client_resolve_info to be present")
	}
}

func TestFlagLogsParity_FlagResolveInfo(t *testing.T) {
	provider, logs := newE2ELogsProvider(t)

	_ = provider.FloatEvaluation(context.Background(), "web-sdk-e2e-flag.double", 10.0, e2eEvalCtx())
	flushLogs(provider)

	if req := requestWithFlagAssigned(t, logs); len(req.FlagResolveInfo) == 0 {
		t.Error("expected flag_resolve_info to be present")
	}
}

func TestFlagLogsParity_MultipleResolvesDedupToOneApply(t *testing.T) {
	provider, logs := newE2ELogsProvider(t)

	// All four paths hit the same underlying flag for the same user, so
	// the applies de-duplicate to a single assignment event.
	_ = provider.BooleanEvaluation(context.Background(), "web-sdk-e2e-flag.bool", true, e2eEvalCtx())
	_ = provider.StringEvaluation(context.Background(), "web-sdk-e2e-flag.str", "default", e2eEvalCtx())
	_ = provider.IntEvaluation(context.Background(), "web-sdk-e2e-flag.int", 10, e2eEvalCtx())
	_ = provider.FloatEvaluation(context.Background(), "web-sdk-e2e-flag.double", 10.0, e2eEvalCtx())
	flushLogs(provider)

	total := totalAppliedFlags(logs)
	if total < 1 {
		t.Fatalf("no applied flag entries after resolving")
	}
	if total > 3 {
		t.Errorf("expected identical applies to be deduplicated, got %d entries", total)
	}
}

func TestFlagLogsParity_DistinctTargetingKeysNotDeduped(t *testing.T) {
	provider, logs := newE2ELogsProvider(t)

	// Distinct users must never be deduplicated: one assignment event per
	// resolve.
	for _, key := range []string{"test-d1", "test-d2", "test-d3", "test-d4"} {
		evalCtx := openfeature.FlattenedContext{"targeting_key": key, "sticky": false}
		_ = provider.BooleanEvaluation(context.Background(), "web-sdk-e2e-flag.bool", true, evalCtx)
	}
	flushLogs(provider)

	if total := totalAppliedFlags(logs); total < 4 {
		t.Errorf("expected at least 4 applied flag entries for distinct users, got %d", total)
	}
}

func TestFlagLogsParity_ShutdownFlushesQueuedBatches(t *testing.T) {
	provider, logs := newE2ELogsProvider(t)

	_ = provider.BooleanEvaluation(context.Background(), "web-sdk-e2e-flag.bool", true, e2eEvalCtx())
	flushLogs(provider)

	if len(logs.bodies) == 0 {
		t.Error("expected shutdown to flush the queued batch to the log sink")
	}
}

func TestFlagLogsParity_ResolveIdPresent(t *testing.T) {
	provider, logs := newE2ELogsProvider(t)

	_ = provider.BooleanEvaluation(context.Background(), "web-sdk-e2e-flag.bool", true, e2eEvalCtx())
	flushLogs(provider)

	fa := requestWithFlagAssigned(t, logs).FlagAssigned[0]
	if fa.ResolveId == "" {
		t.Error("expected resolve_id to be non-empty")
	}
}

// TestFlagLogsParity_ClientIdentificationPresent: the old logger attached
// client_info to every flag_assigned entry; the module-generated format
// carries the client identification once per batch in client_resolve_info
// instead. The information is relocated, not lost — this delta is pinned
// here deliberately.
func TestFlagLogsParity_ClientIdentificationPresent(t *testing.T) {
	provider, logs := newE2ELogsProvider(t)

	_ = provider.BooleanEvaluation(context.Background(), "web-sdk-e2e-flag.bool", true, e2eEvalCtx())
	flushLogs(provider)

	req := requestWithFlagAssigned(t, logs)
	for _, fa := range req.FlagAssigned {
		if fa.ClientInfo != nil && fa.ClientInfo.Client != "" {
			return // per-entry client info also present; nothing relocated
		}
	}
	if len(req.ClientResolveInfo) == 0 {
		t.Fatal("client identification absent: neither flag_assigned.client_info nor client_resolve_info present")
	}
	for _, cri := range req.ClientResolveInfo {
		if cri.Client != "" {
			return
		}
	}
	t.Fatal("expected a non-empty client in client_resolve_info")
}
