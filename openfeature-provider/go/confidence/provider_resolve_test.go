package confidence

import (
	"context"
	"log/slog"
	"os"
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
	lr "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/local_resolver"
	adminv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/admin"
	iamv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/admin"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
	tu "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/structpb"
)

func TestLocalResolverProvider_ReturnsDefaultOnError(t *testing.T) {
	ctx := context.Background()

	// Create minimal state with wrong client secret
	state := &adminv1.ResolverState{
		Flags: []*adminv1.Flag{},
		ClientCredentials: []*iamv1.ClientCredential{
			{
				Credential: &iamv1.ClientCredential_ClientSecret_{
					ClientSecret: &iamv1.ClientCredential_ClientSecret{
						Secret: "wrong-secret",
					},
				},
			},
		},
	}
	stateBytes, _ := proto.Marshal(state)

	stateProvider := &tu.StateProviderMock{
		State:     stateBytes,
		AccountID: "test-account",
	}
	mockFlagLogger := &tu.MockFlagLogger{}
	unsupportedMatStore := newUnsupportedMaterializationStore()

	resolverSupplier := wrapResolverSupplierWithMaterializations(func(ctx context.Context, logSink lr.LogSink) lr.LocalResolver {
		return lr.NewLocalResolverWithPoolSize(ctx, logSink, 2)
	}, unsupportedMatStore)
	// Use different client secret that won't match
	openfeature.SetProviderAndWait(NewLocalResolverProvider(resolverSupplier, stateProvider, mockFlagLogger, "test-secret", slog.New(slog.NewTextHandler(os.Stderr, nil))))
	client := openfeature.NewClient("test-client")

	evalCtx := openfeature.NewTargetlessEvaluationContext(map[string]interface{}{
		"user_id": "test-user",
	})
	t.Run("StringEvaluation returns default on error", func(t *testing.T) {
		defaultValue := "default-value"
		result, err := client.StringValueDetails(ctx, "non-existent-flag.field", defaultValue, evalCtx)
		// expect the error to be non-nil
		if err == nil {
			t.Errorf("Expected error during StringValueDetails, got nil")
		}
		expected := "error code: GENERAL: resolve failed: client secret not found: requested=tes...ret, available=[]"
		if err.Error() != expected {
			t.Errorf("Expected error message %q, got %q", expected, err.Error())
		}

		if result.Value != defaultValue {
			t.Errorf("Expected default value %v, got %v", defaultValue, result.Value)
		}

		if result.Reason != openfeature.ErrorReason {
			t.Errorf("Expected ErrorReason, got %v", result.Reason)
		}

		t.Logf("✓ StringEvaluation correctly returned default value: %s", defaultValue)
	})
}

func TestLocalResolverProvider_ReturnsCorrectValue(t *testing.T) {
	ctx := context.Background()

	// Load real test state
	testState := tu.LoadTestResolverState(t)
	testAcctID := tu.LoadTestAccountID(t)

	stateProvider := &tu.StateProviderMock{
		State:     testState,
		AccountID: testAcctID,
	}
	mockFlagLogger := &tu.MockFlagLogger{}
	unsupportedMatStore := newUnsupportedMaterializationStore()

	resolverSupplier := wrapResolverSupplierWithMaterializations(func(ctx context.Context, logSink lr.LogSink) lr.LocalResolver {
		return lr.NewLocalResolverWithPoolSize(ctx, logSink, 2)
	}, unsupportedMatStore)
	// Use the correct client secret from test data
	openfeature.SetProviderAndWait(NewLocalResolverProvider(resolverSupplier, stateProvider, mockFlagLogger, tu.TestClientSecret, slog.New(slog.NewTextHandler(os.Stderr, nil))))
	client := openfeature.NewClient("test-client")

	evalCtx := openfeature.NewTargetlessEvaluationContext(map[string]interface{}{
		"visitor_id": "tutorial_visitor",
	})

	t.Run("StringEvaluation returns correct variant value", func(t *testing.T) {
		defaultValue := "default-message"
		result, error := client.StringValueDetails(ctx, "tutorial-feature.message", defaultValue, evalCtx)
		if error != nil {
			t.Errorf("Error during StringValueDetails: %v", error)
		}
		// The exciting-welcome variant has a specific message
		expectedMessage := "We are very excited to welcome you to Confidence! This is a message from the tutorial flag."

		if result.Value != expectedMessage {
			t.Errorf("Expected value '%s', got '%s'", expectedMessage, result.Value)
		}

		if result.Reason != openfeature.TargetingMatchReason {
			t.Errorf("Expected TargetingMatchReason, got %v", result.Reason)
		}

	})

	t.Run("ObjectEvaluation returns correct variant structure", func(t *testing.T) {
		defaultValue := map[string]interface{}{
			"message": "default",
			"title":   "default",
		}
		result, error := client.ObjectValueDetails(ctx, "tutorial-feature", defaultValue, evalCtx)
		if error != nil {
			t.Errorf("Error during ObjectValueDetails: %v", error)
		}

		if result.Value == nil {
			t.Fatal("Expected result value to not be nil")
		}

		resultMap, ok := result.Value.(map[string]interface{})
		if !ok {
			t.Fatalf("Expected result value to be a map, got %T", result.Value)
		}

		expectedMessage := "We are very excited to welcome you to Confidence! This is a message from the tutorial flag."
		expectedTitle := "Welcome to Confidence!"

		if resultMap["message"] != expectedMessage {
			t.Errorf("Expected message '%s', got '%v'", expectedMessage, resultMap["message"])
		}

		if resultMap["title"] != expectedTitle {
			t.Errorf("Expected title '%s', got '%v'", expectedTitle, resultMap["title"])
		}

		if result.Reason != openfeature.TargetingMatchReason {
			t.Errorf("Expected TargetingMatchReason, got %v", result.Reason)
		}
	})
}

func TestLocalResolverProvider_DisableExposureCollectionContextKey(t *testing.T) {
	ctx := context.Background()

	testState := tu.LoadTestResolverState(t)
	testAcctID := tu.LoadTestAccountID(t)

	stateProvider := &tu.StateProviderMock{
		State:     testState,
		AccountID: testAcctID,
	}
	mockFlagLogger := &tu.MockFlagLogger{}
	unsupportedMatStore := newUnsupportedMaterializationStore()

	resolverSupplier := wrapResolverSupplierWithMaterializations(func(ctx context.Context, logSink lr.LogSink) lr.LocalResolver {
		return lr.NewLocalResolverWithPoolSize(ctx, logSink, 2)
	}, unsupportedMatStore)
	openfeature.SetProviderAndWait(NewLocalResolverProvider(resolverSupplier, stateProvider, mockFlagLogger, tu.TestClientSecret, slog.New(slog.NewTextHandler(os.Stderr, nil))))
	client := openfeature.NewClient("test-client")

	evalCtx := openfeature.NewTargetlessEvaluationContext(map[string]interface{}{
		"visitor_id":             "tutorial_visitor",
		"_confidence_skip_apply": true,
	})

	t.Run("resolves correctly with _confidence_skip_apply in context", func(t *testing.T) {
		result, err := client.StringValueDetails(ctx, "tutorial-feature.message", "default-message", evalCtx)
		if err != nil {
			t.Errorf("Error during StringValueDetails: %v", err)
		}
		expectedMessage := "We are very excited to welcome you to Confidence! This is a message from the tutorial flag."
		if result.Value != expectedMessage {
			t.Errorf("Expected value '%s', got '%s'", expectedMessage, result.Value)
		}
		if result.Reason != openfeature.TargetingMatchReason {
			t.Errorf("Expected TargetingMatchReason, got %v", result.Reason)
		}
	})
}

func TestLocalResolverProvider_DisableExposureCollectionConfig(t *testing.T) {
	ctx := context.Background()
	mockedResolver := &tu.MockedLocalResolver{
		Response: &wasm.ResolveProcessResponse{
			Result: &wasm.ResolveProcessResponse_Resolved_{
				Resolved: &wasm.ResolveProcessResponse_Resolved{
					Response: tu.CreateTutorialFeatureResponse(),
				},
			},
		},
	}
	stateProvider := &tu.StateProviderMock{
		State:     tu.LoadTestResolverState(t),
		AccountID: tu.LoadTestAccountID(t),
	}
	mockFlagLogger := &tu.MockFlagLogger{}
	unsupportedMatStore := newUnsupportedMaterializationStore()
	resolverSupplier := wrapResolverSupplierWithMaterializations(func(ctx context.Context, logSink lr.LogSink) lr.LocalResolver {
		return mockedResolver
	}, unsupportedMatStore)
	provider := NewLocalResolverProvider(
		resolverSupplier,
		stateProvider,
		mockFlagLogger,
		tu.TestClientSecret,
		slog.New(slog.NewTextHandler(os.Stderr, nil)),
		WithDisableExposureCollection(),
	)
	if err := openfeature.SetProviderAndWait(provider); err != nil {
		t.Fatalf("SetProviderAndWait: %v", err)
	}
	client := openfeature.NewClient("skip-apply-config-test")
	evalCtx := openfeature.NewTargetlessEvaluationContext(map[string]interface{}{
		"visitor_id": "tutorial_visitor",
	})
	if _, err := client.BooleanValueDetails(ctx, "tutorial-feature.enabled", false, evalCtx); err != nil {
		t.Fatalf("resolve: %v", err)
	}
	if mockedResolver.LastRequest == nil {
		t.Fatal("expected ResolveProcess to be called")
	}
	req := mockedResolver.LastRequest.GetWithoutMaterializations()
	if req == nil {
		req = mockedResolver.LastRequest.GetDeferredMaterializations()
	}
	if req == nil {
		t.Fatalf("unexpected resolve request: %#v", mockedResolver.LastRequest)
	}
	if req.Apply {
		t.Fatal("expected apply=false when DisableExposureCollection is configured")
	}
	if mockedResolver.LastSetResolverState == nil {
		t.Fatal("expected SetResolverState to be called")
	}
	if !mockedResolver.LastSetResolverState.DisableExposureCollection {
		t.Fatal("expected disable_exposure_collection=true on SetResolverState when DisableExposureCollection is configured")
	}
}

func TestLocalResolverProvider_PathNotFound(t *testing.T) {
	ctx := context.Background()
	runtime := lr.DefaultResolverFactory(lr.NoOpLogSink, lr.LocalResolverConfig{})
	defer runtime.Close(ctx)

	// Load real test state
	testState := tu.LoadTestResolverState(t)
	testAcctID := tu.LoadTestAccountID(t)

	stateProvider := &tu.StateProviderMock{
		State:     testState,
		AccountID: testAcctID,
	}

	mockFlagLogger := &tu.MockFlagLogger{}
	unsupportedMatStore := newUnsupportedMaterializationStore()

	resolverSupplier := wrapResolverSupplierWithMaterializations(func(ctx context.Context, logSink lr.LogSink) lr.LocalResolver {
		return lr.NewLocalResolverWithPoolSize(ctx, logSink, 2)
	}, unsupportedMatStore)
	// Use the correct client secret from test data
	openfeature.SetProviderAndWait(NewLocalResolverProvider(resolverSupplier, stateProvider, mockFlagLogger, tu.TestClientSecret, slog.New(slog.NewTextHandler(os.Stderr, nil))))
	client := openfeature.NewClient("test-client")

	evalCtx := openfeature.NewTargetlessEvaluationContext(map[string]interface{}{
		"visitor_id": "tutorial_visitor",
	})

	t.Run("Returns FLAG_NOT_FOUND when path does not exist in flag", func(t *testing.T) {
		defaultValue := "default-value"
		// tutorial-feature exists, but "nonexistent" path does not
		result, err := client.StringValueDetails(ctx, "tutorial-feature.nonexistent", defaultValue, evalCtx)

		if err == nil {
			t.Error("Expected error when path not found, got nil")
		} else if err.Error() != "error code: FLAG_NOT_FOUND: path 'nonexistent' not found in flag 'tutorial-feature'" {
			t.Errorf("Expected FLAG_NOT_FOUND error, got: %v", err.Error())
		}

		if result.Value != defaultValue {
			t.Errorf("Expected default value %v, got %v", defaultValue, result.Value)
		}

		if result.Reason != openfeature.ErrorReason {
			t.Errorf("Expected ErrorReason, got %v", result.Reason)
		}

		t.Logf("✓ Correctly returned FLAG_NOT_FOUND for non-existent path")
	})

	t.Run("Returns FLAG_NOT_FOUND when deep path does not exist", func(t *testing.T) {
		defaultValue := "default-value"
		// tutorial-feature.message exists, but message.deeply.nested does not
		result, err := client.StringValueDetails(ctx, "tutorial-feature.message.deeply.nested", defaultValue, evalCtx)

		if err == nil {
			t.Error("Expected error when deep path not found, got nil")
		} else if err.Error() != "error code: FLAG_NOT_FOUND: path 'message.deeply.nested' not found in flag 'tutorial-feature'" {
			t.Errorf("Expected FLAG_NOT_FOUND error for deep path, got: %v", err.Error())
		}

		if result.Value != defaultValue {
			t.Errorf("Expected default value %v, got %v", defaultValue, result.Value)
		}

		t.Logf("✓ Correctly returned FLAG_NOT_FOUND for non-existent deep path")
	})
}

func TestLocalResolverProvider_MissingMaterializations(t *testing.T) {
	ctx := context.Background()

	t.Run("Provider returns resolved value for flag without sticky rules", func(t *testing.T) {

		// Load real test state
		testState := tu.LoadTestResolverState(t)
		testAcctID := tu.LoadTestAccountID(t)

		stateProvider := &tu.StateProviderMock{
			State:     testState,
			AccountID: testAcctID,
		}
		mockFlagLogger := &tu.MockFlagLogger{}
		unsupportedMatStore := newUnsupportedMaterializationStore()

		resolverSupplier := wrapResolverSupplierWithMaterializations(func(ctx context.Context, logSink lr.LogSink) lr.LocalResolver {
			return lr.NewLocalResolverWithPoolSize(ctx, logSink, 2)
		}, unsupportedMatStore)
		openfeature.SetProviderAndWait(NewLocalResolverProvider(resolverSupplier, stateProvider, mockFlagLogger, tu.TestClientSecret, slog.New(slog.NewTextHandler(os.Stderr, nil))))
		client := openfeature.NewClient("test-client")

		evalCtx := openfeature.NewTargetlessEvaluationContext(map[string]interface{}{
			"visitor_id": "tutorial_visitor",
		})

		// The tutorial-feature flag in the test data doesn't have materialization requirements
		// so resolving with empty materializations should succeed
		defaultValue := "default"
		result, error := client.StringValueDetails(ctx, "tutorial-feature.message", defaultValue, evalCtx)
		if error != nil {
			t.Errorf("Error during StringValueDetails: %v", error)
		}

		if result.Value == defaultValue {
			t.Error("Expected resolved value, got default value")
		}

		if result.Reason != openfeature.TargetingMatchReason {
			t.Errorf("Expected TargetingMatchReason, got %v", result.Reason)
		}
	})

	t.Run("Provider returns missing materializations error message for UnsupportedMaterializationStore", func(t *testing.T) {

		// Create state with a flag that requires materializations
		stickyState := tu.CreateStateWithStickyFlag()
		accountId := "test-account"

		stateProvider := &tu.StateProviderMock{
			State:     stickyState,
			AccountID: accountId,
		}
		mockFlagLogger := &tu.MockFlagLogger{}

		unsupportedMatStore := newUnsupportedMaterializationStore()

		resolverSupplier := wrapResolverSupplierWithMaterializations(func(ctx context.Context, logSink lr.LogSink) lr.LocalResolver {
			return lr.NewLocalResolverWithPoolSize(ctx, logSink, 2)
		}, unsupportedMatStore)
		openfeature.SetProviderAndWait(NewLocalResolverProvider(resolverSupplier, stateProvider, mockFlagLogger, "test-secret", slog.New(slog.NewTextHandler(os.Stderr, nil))))
		client := openfeature.NewClient("test-client")

		evalCtx := openfeature.NewTargetlessEvaluationContext(map[string]interface{}{
			"user_id": "test-user-123",
		})

		defaultValue := false
		result, error := client.BooleanValueDetails(ctx, "sticky-test-flag.enabled", defaultValue, evalCtx)
		if error == nil {
			t.Error("Expected error when materializations missing, got nil")
		} else if error.Error() != "error code: GENERAL: flag 'sticky-test-flag' requires materializations; configure a materialization store" {
			t.Errorf("Expected materialization store error, got: %v", error.Error())
		}

		if result.Value != defaultValue {
			t.Errorf("Expected default value %v when materializations missing, got %v", defaultValue, result.Value)
		}

		if result.Reason != openfeature.ErrorReason {
			t.Errorf("Expected ErrorReason when materializations missing, got %v", result.Reason)
		}
	})
}

func TestLocalResolverProvider_WithApplyTime(t *testing.T) {
	applyTime := time.Date(2026, 7, 29, 11, 0, 0, 0, time.UTC)
	ctx := WithApplyTime(context.Background(), applyTime)

	response := func(shouldApply bool) *resolver.ResolveFlagsResponse {
		return &resolver.ResolveFlagsResponse{
			ResolvedFlags: []*resolver.ResolvedFlag{{
				Flag:    "flags/tutorial-feature",
				Variant: "flags/tutorial-feature/variants/on",
				Value: &structpb.Struct{Fields: map[string]*structpb.Value{
					"enabled": structpb.NewBoolValue(true),
				}},
				ShouldApply: shouldApply,
			}},
			ResolveToken: []byte("test-resolve-token"),
			ResolveId:    "test-resolve-id",
		}
	}

	setup := func(t *testing.T, resp *resolver.ResolveFlagsResponse, opts ...Option) (*tu.MockedLocalResolver, *openfeature.Client) {
		t.Helper()
		mockedResolver := &tu.MockedLocalResolver{
			Response: &wasm.ResolveProcessResponse{
				Result: &wasm.ResolveProcessResponse_Resolved_{
					Resolved: &wasm.ResolveProcessResponse_Resolved{Response: resp},
				},
			},
		}
		stateProvider := &tu.StateProviderMock{
			State:     tu.LoadTestResolverState(t),
			AccountID: tu.LoadTestAccountID(t),
		}
		resolverSupplier := wrapResolverSupplierWithMaterializations(func(ctx context.Context, logSink lr.LogSink) lr.LocalResolver {
			return mockedResolver
		}, newUnsupportedMaterializationStore())
		provider := NewLocalResolverProvider(resolverSupplier, stateProvider, &tu.MockFlagLogger{}, tu.TestClientSecret, slog.New(slog.NewTextHandler(os.Stderr, nil)), opts...)
		if err := openfeature.SetProviderAndWait(provider); err != nil {
			t.Fatalf("SetProviderAndWait: %v", err)
		}
		return mockedResolver, openfeature.NewClient("apply-time-test")
	}

	resolveRequest := func(t *testing.T, m *tu.MockedLocalResolver) *resolver.ResolveFlagsRequest {
		t.Helper()
		if m.LastRequest == nil {
			t.Fatal("expected ResolveProcess to be called")
		}
		req := m.LastRequest.GetWithoutMaterializations()
		if req == nil {
			req = m.LastRequest.GetDeferredMaterializations()
		}
		if req == nil {
			t.Fatalf("unexpected resolve request: %#v", m.LastRequest)
		}
		return req
	}

	evalCtx := openfeature.NewTargetlessEvaluationContext(map[string]interface{}{
		"visitor_id": "tutorial_visitor",
	})

	t.Run("backdates the exposure to the provided apply time", func(t *testing.T) {
		m, client := setup(t, response(true))
		result, err := client.BooleanValueDetails(ctx, "tutorial-feature.enabled", false, evalCtx)
		if err != nil {
			t.Fatalf("resolve: %v", err)
		}
		if !result.Value {
			t.Error("expected flag value true")
		}
		if resolveRequest(t, m).Apply {
			t.Error("expected apply=false on the resolve when an apply time is set")
		}
		if m.LastApplyRequest == nil {
			t.Fatal("expected ApplyFlags to be called")
		}
		if got := string(m.LastApplyRequest.ResolveToken); got != "test-resolve-token" {
			t.Errorf("expected the resolve token from the resolve response, got %q", got)
		}
		if len(m.LastApplyRequest.Flags) != 1 {
			t.Fatalf("expected 1 applied flag, got %d", len(m.LastApplyRequest.Flags))
		}
		applied := m.LastApplyRequest.Flags[0]
		if applied.Flag != "flags/tutorial-feature" {
			t.Errorf("expected applied flag 'flags/tutorial-feature', got %q", applied.Flag)
		}
		if !applied.ApplyTime.AsTime().Equal(applyTime) {
			t.Errorf("expected apply_time %v, got %v", applyTime, applied.ApplyTime.AsTime())
		}
		if m.LastApplyRequest.SendTime.AsTime().Equal(applyTime) {
			t.Error("expected send_time to be the current time, not the backdated apply time")
		}
	})

	t.Run("_confidence_skip_apply wins", func(t *testing.T) {
		m, client := setup(t, response(true))
		skipCtx := openfeature.NewTargetlessEvaluationContext(map[string]interface{}{
			"visitor_id":             "tutorial_visitor",
			"_confidence_skip_apply": true,
		})
		if _, err := client.BooleanValueDetails(ctx, "tutorial-feature.enabled", false, skipCtx); err != nil {
			t.Fatalf("resolve: %v", err)
		}
		if resolveRequest(t, m).Apply {
			t.Error("expected apply=false when _confidence_skip_apply is set")
		}
		if m.LastApplyRequest != nil {
			t.Error("expected no ApplyFlags call when _confidence_skip_apply is set")
		}
	})

	t.Run("DisableExposureCollection wins", func(t *testing.T) {
		m, client := setup(t, response(true), WithDisableExposureCollection())
		if _, err := client.BooleanValueDetails(ctx, "tutorial-feature.enabled", false, evalCtx); err != nil {
			t.Fatalf("resolve: %v", err)
		}
		if resolveRequest(t, m).Apply {
			t.Error("expected apply=false when DisableExposureCollection is configured")
		}
		if m.LastApplyRequest != nil {
			t.Error("expected no ApplyFlags call when DisableExposureCollection is configured")
		}
	})

	t.Run("no apply when the resolver sets should_apply=false", func(t *testing.T) {
		m, client := setup(t, response(false))
		if _, err := client.BooleanValueDetails(ctx, "tutorial-feature.enabled", false, evalCtx); err != nil {
			t.Fatalf("resolve: %v", err)
		}
		if m.LastApplyRequest != nil {
			t.Error("expected no ApplyFlags call when should_apply=false")
		}
	})
}
