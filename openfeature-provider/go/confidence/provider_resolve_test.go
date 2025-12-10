package confidence

import (
	"context"
	"log/slog"
	"os"
	"testing"

	"github.com/open-feature/go-sdk/openfeature"
	lr "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/local_resolver"
	tu "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
	adminv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/admin"
	iamv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/admin"
	"google.golang.org/protobuf/proto"
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

	resolverSupplier := wrapResolverSupplierWithMaterializations(lr.NewLocalResolver, unsupportedMatStore)
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
		if err.Error() != "error code: GENERAL: resolve failed: client secret not found" {
			t.Errorf("Expected specific error message during StringValueDetails, got %v", err.Error())
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

	resolverSupplier := wrapResolverSupplierWithMaterializations(lr.NewLocalResolver, unsupportedMatStore)
	// Use the correct client secret from test data
	openfeature.SetProviderAndWait(NewLocalResolverProvider(resolverSupplier, stateProvider, mockFlagLogger, "mkjJruAATQWjeY7foFIWfVAcBWnci2YF", slog.New(slog.NewTextHandler(os.Stderr, nil))))
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

func TestLocalResolverProvider_PathNotFound(t *testing.T) {
	ctx := context.Background()
	runtime := lr.DefaultResolverFactory(lr.NoOpLogSink)
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

	resolverSupplier := wrapResolverSupplierWithMaterializations(lr.NewLocalResolver, unsupportedMatStore)
	// Use the correct client secret from test data
	openfeature.SetProviderAndWait(NewLocalResolverProvider(resolverSupplier, stateProvider, mockFlagLogger, "mkjJruAATQWjeY7foFIWfVAcBWnci2YF", slog.New(slog.NewTextHandler(os.Stderr, nil))))
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

		resolverSupplier := wrapResolverSupplierWithMaterializations(lr.NewLocalResolver, unsupportedMatStore)
		openfeature.SetProviderAndWait(NewLocalResolverProvider(resolverSupplier, stateProvider, mockFlagLogger, "mkjJruAATQWjeY7foFIWfVAcBWnci2YF", slog.New(slog.NewTextHandler(os.Stderr, nil))))
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

		resolverSupplier := wrapResolverSupplierWithMaterializations(lr.NewLocalResolver, unsupportedMatStore)
		openfeature.SetProviderAndWait(NewLocalResolverProvider(resolverSupplier, stateProvider, mockFlagLogger, "test-secret", slog.New(slog.NewTextHandler(os.Stderr, nil))))
		client := openfeature.NewClient("test-client")

		evalCtx := openfeature.NewTargetlessEvaluationContext(map[string]interface{}{
			"user_id": "test-user-123",
		})

		defaultValue := false
		result, error := client.BooleanValueDetails(ctx, "sticky-test-flag.enabled", defaultValue, evalCtx)
		if error == nil {
			t.Error("Expected error when materializations missing, got nil")
		} else if error.Error() != "error code: GENERAL: resolve failed: failed to handle missing materializations: materialization read not supported, falling back to remote resolution" {
			t.Errorf("Expected 'GENERAL: resolve failed: failed to handle missing materializations: materialization read not supported, falling back to remote resolution', got: %v", error.Error())
		}

		if result.Value != defaultValue {
			t.Errorf("Expected default value %v when materializations missing, got %v", defaultValue, result.Value)
		}

		if result.Reason != openfeature.ErrorReason {
			t.Errorf("Expected ErrorReason when materializations missing, got %v", result.Reason)
		}
	})
}
