package confidence

import (
	"context"
	"log/slog"
	"os"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
	fl "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/flag_logger"
	lr "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/local_resolver"
	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	tu "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
	"google.golang.org/grpc"
)

// mockStateProvider provides test state for integration testing
type mockStateProvider struct {
	state     []byte
	accountID string
}

func (m *mockStateProvider) Provide(ctx context.Context) ([]byte, string, error) {
	accountID := m.accountID
	if accountID == "" {
		accountID = "test-account"
	}
	return m.state, accountID, nil
}

// trackingFlagLogger wraps a real GrpcWasmFlagLogger with a mocked connection
type trackingFlagLogger struct {
	actualLogger   FlagLogger
	logsSentCount  int32
	shutdownCalled bool
	mu             sync.Mutex
	// Track when async operations complete
	lastWriteCompleted chan struct{}
}

func (t *trackingFlagLogger) Write(request *resolverv1.WriteFlagLogsRequest) {
	atomic.AddInt32(&t.logsSentCount, int32(len(request.FlagAssigned)))
	t.actualLogger.Write(request)
}

func (t *trackingFlagLogger) Shutdown() {
	t.mu.Lock()
	t.shutdownCalled = true
	t.mu.Unlock()
	t.actualLogger.Shutdown()
}

func (t *trackingFlagLogger) GetLogsSentCount() int32 {
	return atomic.LoadInt32(&t.logsSentCount)
}

func (t *trackingFlagLogger) WasShutdownCalled() bool {
	t.mu.Lock()
	defer t.mu.Unlock()
	return t.shutdownCalled
}

// mockGrpcStubForIntegration provides a mock gRPC stub that tracks async operations
type mockGrpcStubForIntegration struct {
	resolverv1.InternalFlagLoggerServiceClient
	callsReceived  int32
	onCallReceived chan struct{}
}

func (m *mockGrpcStubForIntegration) ClientWriteFlagLogs(ctx context.Context, req *resolverv1.WriteFlagLogsRequest, opts ...grpc.CallOption) (*resolverv1.WriteFlagLogsResponse, error) {
	atomic.AddInt32(&m.callsReceived, 1)
	// Signal that a call was received
	select {
	case m.onCallReceived <- struct{}{}:
	default:
	}
	// Simulate some processing time to verify shutdown waits for completion
	time.Sleep(50 * time.Millisecond)
	return &resolverv1.WriteFlagLogsResponse{}, nil
}

func (m *mockGrpcStubForIntegration) GetCallsReceived() int32 {
	return atomic.LoadInt32(&m.callsReceived)
}

// TestIntegration_OpenFeatureShutdownFlushesLogs tests the full integration:
// - Real OpenFeature SDK
// - Real provider with all components
// - Mock state provider (using test state)
// - Actual GrpcWasmFlagLogger with mocked gRPC connection
// - Verifies logs are flushed and gRPC calls complete on openfeature.Shutdown()
// This test specifically verifies the shutdown bug fix where the GrpcWasmFlagLogger's
// async goroutines complete before Shutdown() returns, ensuring no data loss.
func TestIntegration_OpenFeatureShutdownFlushesLogs(t *testing.T) {
	// Load test state
	testState := tu.LoadTestResolverState(t)
	accountID := tu.LoadTestAccountID(t)

	ctx := context.Background()

	// Create mock state provider
	stateProvider := &mockStateProvider{
		state: testState,
	}

	// Create tracking logger with actual GrpcWasmFlagLogger and mocked connection
	mockStub := &mockGrpcStubForIntegration{
		onCallReceived: make(chan struct{}, 100), // Buffer to prevent blocking
	}
	actualGrpcLogger := fl.NewGrpcWasmFlagLogger(mockStub, "test-client-secret", slog.New(slog.NewTextHandler(os.Stderr, nil)))

	trackingLogger := &trackingFlagLogger{
		actualLogger:       actualGrpcLogger,
		lastWriteCompleted: make(chan struct{}, 1),
	}

	// Create provider with test state
	provider, err := createProviderWithTestState(ctx, stateProvider, accountID, trackingLogger, newUnsupportedMaterializationStore())
	if err != nil {
		t.Fatalf("Failed to create provider: %v", err)
	}

	client := openfeature.NewClient("integration-test")

	state := client.State()
	if state != "NOT_READY" {
		t.Fatalf("Expected client state to be NOT_READY before initialization, was %+v", state)
	}
	// Register with OpenFeature
	err = openfeature.SetProviderAndWait(provider)
	if err != nil {
		t.Fatalf("Failed to set provider: %v", err)
	}

	evalCtx := openfeature.NewEvaluationContext(
		"tutorial_visitor",
		map[string]interface{}{
			"visitor_id": "tutorial_visitor",
		},
	)

	state = client.State()
	if state != "READY" {
		t.Fatalf("Expected client state to be READY after initialization, was %+v", state)
	}

	// Evaluate the tutorial-feature flag (this should generate logs)
	// This flag exists in the test state and should resolve successfully
	numEvaluations := 5
	for i := 0; i < numEvaluations; i++ {
		result, _ := client.ObjectValueDetails(ctx, "tutorial-feature", map[string]interface{}{}, evalCtx)
		if i == 0 {
			t.Logf("First evaluation result: %+v", result)
		}
	}

	// Now shutdown - this should flush all logs
	openfeature.Shutdown()

	// Verify shutdown was called
	if !trackingLogger.WasShutdownCalled() {
		t.Error("Expected logger shutdown to be called")
	}

	// Verify logs were flushed
	finalLogCount := trackingLogger.GetLogsSentCount()
	if finalLogCount == 0 {
		t.Error("Expected logs to be flushed during shutdown, but no logs were sent")
	}

	// Verify that the mock gRPC connection actually received the calls
	// This proves that the connection completed before shutdown returned
	grpcCallsReceived := mockStub.GetCallsReceived()
	if grpcCallsReceived == 0 {
		t.Error("Expected mock gRPC connection to receive calls, but none were received")
	}

	t.Logf("Successfully flushed %d log entries via %d gRPC calls during shutdown", finalLogCount, grpcCallsReceived)
}

func TestIntegration_OpenFeatureResolveStickyFlagMatStoreReadAndWrite(t *testing.T) {
	ctx := context.Background()

	// Create mock state provider
	stateProvider := &mockStateProvider{
		state:     tu.CreateStateWithStickyFlag(),
		accountID: "test-account",
	}

	// Create tracking logger with actual GrpcWasmFlagLogger and mocked connection
	mockStub := &mockGrpcStubForIntegration{
		onCallReceived: make(chan struct{}, 100), // Buffer to prevent blocking
	}
	actualGrpcLogger := fl.NewGrpcWasmFlagLogger(mockStub, "test-secret", slog.New(slog.NewTextHandler(os.Stderr, nil)))

	trackingLogger := &trackingFlagLogger{
		actualLogger:       actualGrpcLogger,
		lastWriteCompleted: make(chan struct{}, 1),
	}
	matStore := newInMemoryMaterializationStore(nil)
	resolverSupplier := wrapResolverSupplierWithMaterializations(lr.NewLocalResolver, matStore)

	// Create provider with test state
	provider := NewLocalResolverProvider(resolverSupplier, stateProvider, trackingLogger, "test-secret", slog.New(slog.NewTextHandler(os.Stderr, nil)), 0, 0)

	client := openfeature.NewClient("integration-test")

	state := client.State()
	if state != "NOT_READY" {
		t.Fatalf("Expected client state to be NOT_READY before initialization, was %+v", state)
	}
	// Register with OpenFeature
	err := openfeature.SetProviderAndWait(provider)
	if err != nil {
		t.Fatalf("Failed to set provider: %v", err)
	}

	evalCtx := openfeature.NewEvaluationContext(
		"test-user-123",
		map[string]interface{}{
			"user_id": "test-user-123",
		},
	)

	state = client.State()
	if state != "READY" {
		t.Fatalf("Expected client state to be READY after initialization, was %+v", state)
	}

	// Evaluate the tutorial-feature flag (this should generate logs)
	// This flag exists in the test state and should resolve successfully
	result, _ := client.BooleanValueDetails(ctx, "sticky-test-flag.enabled", false, evalCtx)
	t.Logf("Sticky flag evaluation result: %+v", result)

	// Verify result is as expected
	if result.Value != true {
		t.Fatalf("Expected sticky flag result to be true, got %+v", result.Value)
	}

	if got, want := result.Variant, "flags/sticky-test-flag/variants/on"; got != want {
		t.Fatalf("Unexpected variant: got %q want %q", got, want)
	}

	// Verify that the materialization store was read
	if len(matStore.ReadCalls()) != 1 {
		t.Fatalf("expected 1 read call to materialization store, got %d", len(matStore.ReadCalls()))
	}
	readOps := matStore.ReadCalls()[0]
	if len(readOps) != 1 {
		t.Fatalf("expected 1 read op in materialization store call, got %d", len(readOps))
	}
	readOp, ok := readOps[0].(*ReadOpVariant)
	if !ok {
		t.Fatalf("expected ReadOpVariant type, got %T", readOps[0])
	}
	if got, want := readOp.Materialization(), "experiment_v1"; got != want {
		t.Fatalf("unexpected materialization id: got %q want %q", got, want)
	}
	if got, want := readOp.Unit(), "test-user-123"; got != want {
		t.Fatalf("unexpected unit id: got %q want %q", got, want)
	}
	if got, want := readOp.Rule(), "flags/sticky-test-flag/rules/sticky-rule"; got != want {
		t.Fatalf("unexpected rule id: got %q want %q", got, want)
	}

	// Verify that the materialization store was written to (storing the variant)
	// requires that we wait for async go routine
	time.Sleep(10 * time.Millisecond) // wait for async write to complete
	if len(matStore.WriteCalls()) != 1 {
		t.Fatalf("expected 1 write call to materialization store, got %d", len(matStore.WriteCalls()))
	}
	writeOps := matStore.WriteCalls()[0]
	if len(writeOps) != 1 {
		t.Fatalf("expected 1 write op in materialization store call, got %d", len(writeOps))
	}
	writeOp, ok := writeOps[0].(*WriteOpVariant)
	if !ok {
		t.Fatalf("expected WriteOpVariant type, got %T", writeOps[0])
	}
	if got, want := writeOp.Materialization(), "experiment_v1"; got != want {
		t.Fatalf("unexpected materialization id: got %q want %q", got, want)
	}
	if got, want := writeOp.Unit(), "test-user-123"; got != want {
		t.Fatalf("unexpected unit id: got %q want %q", got, want)
	}
	if got, want := writeOp.Rule(), "flags/sticky-test-flag/rules/sticky-rule"; got != want {
		t.Fatalf("unexpected rule id: got %q want %q", got, want)
	}
	if got, want := writeOp.Variant(), "flags/sticky-test-flag/variants/on"; got != want {
		t.Fatalf("unexpected variant: got %q want %q", got, want)
	}

	// Now shutdown
	openfeature.Shutdown()
}

func TestIntegration_OpenFeatureMaterializedSegmentCriterion(t *testing.T) {
	ctx := context.Background()

	// Create test state with MaterializedSegmentCriterion
	testState := tu.CreateStateWithMaterializedSegment()

	// Create mock state provider
	stateProvider := &mockStateProvider{
		state:     testState,
		accountID: "test-account",
	}

	SECRET := "test-secret"

	// Create tracking logger
	mockStub := &mockGrpcStubForIntegration{
		onCallReceived: make(chan struct{}, 100),
	}
	actualGrpcLogger := fl.NewGrpcWasmFlagLogger(mockStub, SECRET, slog.New(slog.NewTextHandler(os.Stderr, nil)))

	trackingLogger := &trackingFlagLogger{
		actualLogger:       actualGrpcLogger,
		lastWriteCompleted: make(chan struct{}, 1),
	}

	t.Run("with unit in materialized segment", func(t *testing.T) {
		// Create materialization store with unit in the materialized segment
		initialInclusions := map[string]map[string]bool{
			"tutorial_visitor": {
				"materializedSegments/nicklas-custom-targeting": true,
			},
		}
		matStore := newInMemoryMaterializationStoreWithInclusions(nil, initialInclusions)
		resolverSupplier := wrapResolverSupplierWithMaterializations(lr.NewLocalResolver, matStore)

		// Create provider with test state
		provider := NewLocalResolverProvider(resolverSupplier, stateProvider, trackingLogger, SECRET, slog.New(slog.NewTextHandler(os.Stderr, nil)), 0, 0)

		client := openfeature.NewClient("integration-test-mat-seg")

		// Register with OpenFeature
		err := openfeature.SetProviderAndWait(provider)
		if err != nil {
			t.Fatalf("Failed to set provider: %v", err)
		}

		evalCtx := openfeature.NewEvaluationContext(
			"tutorial_visitor",
			map[string]interface{}{
				"user_id": "tutorial_visitor",
			},
		)

		// Evaluate the custom-targeted-flag
		result, _ := client.StringValueDetails(ctx, "custom-targeted-flag.message", "", evalCtx)
		t.Logf("Evaluation result with unit in materialization: %+v", result)

		// Verify that materialization store was read
		readCalls := matStore.ReadCalls()
		t.Logf("Number of read calls to materialization store: %d", len(readCalls))
		for i, readOps := range readCalls {
			t.Logf("Read call %d: %d operations", i, len(readOps))
			for j, op := range readOps {
				if inclOp, ok := op.(*ReadOpInclusion); ok {
					t.Logf("  Op %d: ReadOpInclusion(unit=%s, mat=%s)", j, inclOp.Unit(), inclOp.Materialization())
				} else if varOp, ok := op.(*ReadOpVariant); ok {
					t.Logf("  Op %d: ReadOpVariant(unit=%s, mat=%s, rule=%s)", j, varOp.Unit(), varOp.Materialization(), varOp.Rule())
				} else {
					t.Logf("  Op %d: Unknown type %T", j, op)
				}
			}
		}

		// Should resolve to cake-exclamation variant
		if got, want := result.Value, "Did someone say CAKE?!"; got != want {
			t.Errorf("Expected message %q, got %q", want, got)
		}
		if got, want := result.Variant, "flags/custom-targeted-flag/variants/cake-exclamation"; got != want {
			t.Errorf("Expected variant %q, got %q", want, got)
		}

		// Now shutdown
		openfeature.Shutdown()
	})

	t.Run("with unit not in materialized segment", func(t *testing.T) {
		// Create materialization store with unit NOT in the materialized segment
		initialInclusions := map[string]map[string]bool{
			"tutorial_visitor": {
				"materializedSegments/nicklas-custom-targeting": false,
			},
		}
		matStore := newInMemoryMaterializationStoreWithInclusions(nil, initialInclusions)
		resolverSupplier := wrapResolverSupplierWithMaterializations(lr.NewLocalResolver, matStore)

		// Create provider with test state
		provider := NewLocalResolverProvider(resolverSupplier, stateProvider, trackingLogger, SECRET, slog.New(slog.NewTextHandler(os.Stderr, nil)), 0, 0)

		client := openfeature.NewClient("integration-test-mat-seg-not-in")

		// Register with OpenFeature
		err := openfeature.SetProviderAndWait(provider)
		if err != nil {
			t.Fatalf("Failed to set provider: %v", err)
		}

		evalCtx := openfeature.NewEvaluationContext(
			"tutorial_visitor",
			map[string]interface{}{
				"user_id": "tutorial_visitor",
			},
		)

		// Evaluate the custom-targeted-flag
		result, _ := client.StringValueDetails(ctx, "custom-targeted-flag.message", "", evalCtx)
		t.Logf("Evaluation result with unit NOT in materialization: %+v", result)

		// Should fall through to default variant
		if got, want := result.Value, "nothing fun"; got != want {
			t.Errorf("Expected message %q, got %q", want, got)
		}
		if got, want := result.Variant, "flags/custom-targeted-flag/variants/default"; got != want {
			t.Errorf("Expected variant %q, got %q", want, got)
		}

		// Now shutdown
		openfeature.Shutdown()
	})

	t.Run("without materialization context", func(t *testing.T) {
		// Create empty materialization store (no context for tutorial_visitor)
		matStore := newInMemoryMaterializationStore(nil)
		resolverSupplier := wrapResolverSupplierWithMaterializations(lr.NewLocalResolver, matStore)

		// Create provider with test state
		provider := NewLocalResolverProvider(resolverSupplier, stateProvider, trackingLogger, SECRET, slog.New(slog.NewTextHandler(os.Stderr, nil)), 0, 0)

		client := openfeature.NewClient("integration-test-mat-seg-no-ctx")

		// Register with OpenFeature
		err := openfeature.SetProviderAndWait(provider)
		if err != nil {
			t.Fatalf("Failed to set provider: %v", err)
		}

		evalCtx := openfeature.NewEvaluationContext(
			"tutorial_visitor",
			map[string]interface{}{
				"user_id": "tutorial_visitor",
			},
		)

		// Evaluate the custom-targeted-flag
		result, _ := client.StringValueDetails(ctx, "custom-targeted-flag.message", "", evalCtx)
		t.Logf("Evaluation result without materialization context: %+v", result)

		// Should fall through to default variant (missing materialization causes rule to not match)
		if got, want := result.Value, "nothing fun"; got != want {
			t.Errorf("Expected message %q, got %q", want, got)
		}
		if got, want := result.Variant, "flags/custom-targeted-flag/variants/default"; got != want {
			t.Errorf("Expected variant %q, got %q", want, got)
		}

		// Now shutdown
		openfeature.Shutdown()
	})
}

// createProviderWithTestState creates a provider with mock state provider and tracking logger
func createProviderWithTestState(
	ctx context.Context,
	stateProvider StateProvider,
	accountID string,
	logger FlagLogger,
	matStore MaterializationStore,
) (*LocalResolverProvider, error) {
	resolverSupplier := wrapResolverSupplierWithMaterializations(lr.NewLocalResolver, matStore)
	// Create provider with the client secret from test state
	// The test state includes client secret: mkjJruAATQWjeY7foFIWfVAcBWnci2YF
	provider := NewLocalResolverProvider(resolverSupplier, stateProvider, logger, "mkjJruAATQWjeY7foFIWfVAcBWnci2YF", slog.New(slog.NewTextHandler(os.Stderr, nil)), 0, 0)
	return provider, nil
}
