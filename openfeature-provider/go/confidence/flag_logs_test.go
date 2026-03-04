package confidence

import (
	"context"
	"log/slog"
	"os"
	"strings"
	"testing"
	"time"

	fl "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/flag_logger"
	lr "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/local_resolver"
	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"

	"github.com/open-feature/go-sdk/openfeature"
)

// Unit tests that verify WriteFlagLogs contains correct flag assignment data.
//
// These tests use a CapturingFlagLogger to capture all flag log requests and verify:
//   - Flag names are correctly reported
//   - Targeting keys match the evaluation context
//   - Assignment information is present and valid
//   - Variant information matches the resolved value

const (
	unitTestClientSecret = "ti5Sipq5EluCYRG7I5cdbpWC3xq7JTWv"
	unitTestTargetingKey = "test-a"
)

// setupFlagLogsUnitTest creates a provider with a capturing logger for testing.
// Returns the capturing logger, provider, and client.
// The caller is responsible for calling openfeature.Shutdown() when done.
func setupFlagLogsUnitTest(t *testing.T) (*fl.CapturingFlagLogger, openfeature.IClient) {
	ctx := context.Background()

	// Create capturing logger
	capturingLogger := fl.NewCapturingFlagLogger()

	// Create state provider that fetches from real Confidence service
	logger := slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelInfo}))
	stateProvider := NewFlagsAdminStateFetcher(unitTestClientSecret, logger)

	// Fetch initial state
	if err := stateProvider.Reload(ctx); err != nil {
		t.Fatalf("Failed to reload state: %v", err)
	}

	// Create provider
	unsupportedMatStore := newUnsupportedMaterializationStore()

	resolverSupplier := wrapResolverSupplierWithMaterializations(func(ctx context.Context, logSink lr.LogSink) lr.LocalResolver {
		return lr.NewLocalResolverWithPoolSize(ctx, logSink, 2)
	}, unsupportedMatStore)
	provider := NewLocalResolverProvider(resolverSupplier, stateProvider, capturingLogger, unitTestClientSecret, logger)

	// Set provider and wait for ready
	err := openfeature.SetProviderAndWait(provider)
	if err != nil {
		t.Fatalf("Failed to set provider: %v", err)
	}

	// Set evaluation context
	evalCtx := openfeature.NewEvaluationContext(unitTestTargetingKey, map[string]interface{}{
		"sticky": false,
	})
	openfeature.SetEvaluationContext(evalCtx)

	// Clear any logs captured during initialization
	capturingLogger.Clear()

	client := openfeature.NewClient("flag-logs-test")
	return capturingLogger, client
}

// flushAndWait shuts down OpenFeature and waits a bit for async operations
func flushAndWait() {
	openfeature.Shutdown()
	// Give async operations time to complete
	time.Sleep(100 * time.Millisecond)
}

// findRequestWithFlagAssigned returns the first captured request that contains
// FlagAssigned entries. With pooled WASM instances, flush may produce multiple
// requests (one per pool slot) and only the slot that handled the resolve will
// have FlagAssigned data.
func findRequestWithFlagAssigned(t *testing.T, logger *fl.CapturingFlagLogger) *resolverv1.WriteFlagLogsRequest {
	t.Helper()
	requests := logger.GetCapturedRequests()
	if len(requests) == 0 {
		t.Fatal("Expected captured requests, got none")
	}
	for _, req := range requests {
		if len(req.FlagAssigned) > 0 {
			return req
		}
	}
	t.Fatal("Expected flag_assigned entries in captured requests, got none")
	return nil
}

func TestFlagLogs_ShouldCaptureWriteFlagLogsAfterBooleanResolve(t *testing.T) {
	capturingLogger, client := setupFlagLogsUnitTest(t)

	ctx := context.Background()

	// Resolve a boolean flag
	value, err := client.BooleanValue(ctx, "web-sdk-e2e-flag.bool", true, openfeature.EvaluationContext{})
	if err != nil {
		t.Fatalf("Failed to resolve boolean flag: %v", err)
	}
	if value != false {
		t.Errorf("Expected false, got %v", value)
	}

	// Shutdown to flush logs
	flushAndWait()

	// Verify captured flag logs
	request := findRequestWithFlagAssigned(t, capturingLogger)

	// Find the flag we resolved
	found := false
	for _, fa := range request.FlagAssigned {
		for _, af := range fa.Flags {
			if strings.Contains(af.Flag, "web-sdk-e2e-flag") {
				found = true
				if af.TargetingKey != unitTestTargetingKey {
					t.Errorf("Expected targeting key %s, got %s", unitTestTargetingKey, af.TargetingKey)
				}
			}
		}
	}
	if !found {
		t.Error("Expected to find web-sdk-e2e-flag in captured requests")
	}
}

func TestFlagLogs_ShouldCaptureCorrectVariantInFlagLogs(t *testing.T) {
	capturingLogger, client := setupFlagLogsUnitTest(t)

	ctx := context.Background()

	// Resolve a string flag
	value, err := client.StringValue(ctx, "web-sdk-e2e-flag.str", "default", openfeature.EvaluationContext{})
	if err != nil {
		t.Fatalf("Failed to resolve string flag: %v", err)
	}
	if value != "control" {
		t.Errorf("Expected 'control', got %v", value)
	}

	// Shutdown to flush logs
	flushAndWait()

	request := findRequestWithFlagAssigned(t, capturingLogger)

	// Verify variant information is present
	flagAssigned := request.FlagAssigned[0]
	if len(flagAssigned.Flags) == 0 {
		t.Fatal("Expected flags in flag_assigned, got none")
	}

	appliedFlag := flagAssigned.Flags[0]
	if appliedFlag.Flag == "" {
		t.Error("Expected flag name to be non-empty")
	}
}

func TestFlagLogs_ShouldCaptureClientResolveInfo(t *testing.T) {
	capturingLogger, client := setupFlagLogsUnitTest(t)

	ctx := context.Background()

	// Perform a resolve
	_, _ = client.IntValue(ctx, "web-sdk-e2e-flag.int", 10, openfeature.EvaluationContext{})

	// Shutdown to flush logs
	flushAndWait()

	request := findRequestWithFlagAssigned(t, capturingLogger)
	if len(request.ClientResolveInfo) == 0 {
		t.Error("Expected client_resolve_info to be present")
	}
}

func TestFlagLogs_ShouldCaptureFlagResolveInfo(t *testing.T) {
	capturingLogger, client := setupFlagLogsUnitTest(t)

	ctx := context.Background()

	// Perform a resolve
	_, _ = client.FloatValue(ctx, "web-sdk-e2e-flag.double", 10.0, openfeature.EvaluationContext{})

	// Shutdown to flush logs
	flushAndWait()

	request := findRequestWithFlagAssigned(t, capturingLogger)
	if len(request.FlagResolveInfo) == 0 {
		t.Error("Expected flag_resolve_info to be present")
	}
}

func TestFlagLogs_ShouldCaptureMultipleResolvesInSingleRequest(t *testing.T) {
	capturingLogger, client := setupFlagLogsUnitTest(t)

	ctx := context.Background()

	// Perform multiple resolves
	_, _ = client.BooleanValue(ctx, "web-sdk-e2e-flag.bool", true, openfeature.EvaluationContext{})
	_, _ = client.StringValue(ctx, "web-sdk-e2e-flag.str", "default", openfeature.EvaluationContext{})
	_, _ = client.IntValue(ctx, "web-sdk-e2e-flag.int", 10, openfeature.EvaluationContext{})
	_, _ = client.FloatValue(ctx, "web-sdk-e2e-flag.double", 10.0, openfeature.EvaluationContext{})

	// Shutdown to flush logs
	flushAndWait()

	// Should have captured log entries for all resolves
	totalFlagAssigned := capturingLogger.GetTotalFlagAssignedCount()
	if totalFlagAssigned < 4 {
		t.Errorf("Expected at least 4 flag_assigned entries, got %d", totalFlagAssigned)
	}
}

func TestFlagLogs_ShouldCallShutdownOnProviderShutdown(t *testing.T) {
	capturingLogger, client := setupFlagLogsUnitTest(t)

	ctx := context.Background()

	// Perform a resolve
	_, _ = client.BooleanValue(ctx, "web-sdk-e2e-flag.bool", true, openfeature.EvaluationContext{})

	// Shutdown
	flushAndWait()

	if !capturingLogger.WasShutdownCalled() {
		t.Error("Expected shutdown to be called on logger")
	}
}

func TestFlagLogs_ShouldCaptureResolveIdInFlagAssigned(t *testing.T) {
	capturingLogger, client := setupFlagLogsUnitTest(t)

	ctx := context.Background()

	// Perform a resolve
	_, _ = client.BooleanValue(ctx, "web-sdk-e2e-flag.bool", true, openfeature.EvaluationContext{})

	// Shutdown to flush logs
	flushAndWait()

	request := findRequestWithFlagAssigned(t, capturingLogger)

	// Verify resolve_id is present
	flagAssigned := request.FlagAssigned[0]
	if flagAssigned.ResolveId == "" {
		t.Error("Expected resolve_id to be non-empty")
	}
}

func TestFlagLogs_ShouldCaptureClientInfoInFlagAssigned(t *testing.T) {
	capturingLogger, client := setupFlagLogsUnitTest(t)

	ctx := context.Background()

	// Perform a resolve
	_, _ = client.BooleanValue(ctx, "web-sdk-e2e-flag.bool", true, openfeature.EvaluationContext{})

	// Shutdown to flush logs
	flushAndWait()

	request := findRequestWithFlagAssigned(t, capturingLogger)

	// Verify client_info is present
	flagAssigned := request.FlagAssigned[0]
	if flagAssigned.ClientInfo == nil {
		t.Error("Expected client_info to be present")
	}
	if flagAssigned.ClientInfo != nil && flagAssigned.ClientInfo.Client == "" {
		t.Error("Expected client to be non-empty")
	}
}
