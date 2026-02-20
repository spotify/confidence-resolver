package local_resolver

import (
	"context"
	"os"
	"testing"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
	tu "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"

	"google.golang.org/protobuf/types/known/structpb"
)

var resolverFactory LocalResolverFactory

func TestMain(m *testing.M) {
	resolverFactory = DefaultResolverFactory(NoOpLogSink)
	defer resolverFactory.Close(context.Background())
	os.Exit(m.Run())
}

func TestSwapWasmResolverApi_NewSwapWasmResolverApi(t *testing.T) {
	ctx := context.Background()

	initialState := tu.CreateMinimalResolverState()
	accountId := "test-account"

	defaultResolver := resolverFactory.New()
	defer defaultResolver.Close(ctx)

	// Initialize with test state
	if err := defaultResolver.SetResolverState(&wasm.SetResolverStateRequest{
		State:     initialState,
		AccountId: accountId,
	}); err != nil {
		t.Fatalf("Failed to initialize defaultResolver with state: %v", err)
	}

	if defaultResolver == nil {
		t.Fatal("Expected non-nil SwapWasmResolverApi")
	}

}

func TestSwapWasmResolverApi_WithRealState(t *testing.T) {
	ctx := context.Background()

	// Load real test state from data directory
	testState := tu.LoadTestResolverState(t)
	testAcctID := tu.LoadTestAccountID(t)

	defaultResolver := resolverFactory.New()
	defer defaultResolver.Close(ctx)

	// Initialize with test state
	if err := defaultResolver.SetResolverState(&wasm.SetResolverStateRequest{
		State:     testState,
		AccountId: testAcctID,
	}); err != nil {
		t.Fatalf("Failed to initialize defaultResolver with state: %v", err)
	}

	request := tu.CreateResolveProcessRequest(tu.CreateTutorialFeatureRequest())

	processResponse, err := defaultResolver.ResolveProcess(request)
	if err != nil {
		t.Fatalf("Unexpected error resolving tutorial-feature flag: %v", err)
	}

	if processResponse == nil {
		t.Fatal("Expected non-nil response")
	}

	response := processResponse.GetResolved().GetResponse()
	if response == nil {
		t.Fatal("Expected successful resolve response")
	}

	if len(response.ResolvedFlags) != 1 {
		t.Fatalf("Expected 1 resolved flag, got %d", len(response.ResolvedFlags))
	}

	resolvedFlag := response.ResolvedFlags[0]

	// Verify the exact flag name
	if resolvedFlag.Flag != "flags/tutorial-feature" {
		t.Errorf("Expected flag 'flags/tutorial-feature', got '%s'", resolvedFlag.Flag)
	}

	// Verify the exact variant
	// The tutorial-visitor segment should resolve to the exciting-welcome variant
	expectedVariant := "flags/tutorial-feature/variants/exciting-welcome"
	if resolvedFlag.Variant != expectedVariant {
		t.Errorf("Expected variant '%s', got '%s'", expectedVariant, resolvedFlag.Variant)
	}

	// Verify the reason is MATCH (successful targeting match)
	if resolvedFlag.Reason.String() != "RESOLVE_REASON_MATCH" {
		t.Errorf("Expected reason RESOLVE_REASON_MATCH, got %v", resolvedFlag.Reason)
	}

	// Verify the resolved value has the expected structure and content
	if resolvedFlag.Value == nil {
		t.Fatal("Expected non-nil value in resolved flag")
	}

	fields := resolvedFlag.Value.GetFields()
	if fields == nil {
		t.Fatal("Expected fields in resolved value")
	}

	// Verify the exact message value from the exciting-welcome variant
	expectedMessage := "We are very excited to welcome you to Confidence! This is a message from the tutorial flag."
	messageValue, hasMessage := fields["message"]
	if !hasMessage {
		t.Error("Expected 'message' field in resolved value")
	} else if messageValue.GetStringValue() != expectedMessage {
		t.Errorf("Expected message '%s', got '%s'", expectedMessage, messageValue.GetStringValue())
	}

	// Verify the exact title value from the exciting-welcome variant
	expectedTitle := "Welcome to Confidence!"
	titleValue, hasTitle := fields["title"]
	if !hasTitle {
		t.Error("Expected 'title' field in resolved value")
	} else if titleValue.GetStringValue() != expectedTitle {
		t.Errorf("Expected title '%s', got '%s'", expectedTitle, titleValue.GetStringValue())
	}

	t.Logf("✓ Successfully resolved flag with correct variant and values")
}

func TestSwapWasmResolverApi_UpdateStateAndFlushLogs(t *testing.T) {
	ctx := context.Background()

	// Load real test state
	initialState := tu.LoadTestResolverState(t)
	accountId := tu.LoadTestAccountID(t)

	defaultResolver := resolverFactory.New()
	defer defaultResolver.Close(ctx)

	// Initialize with test state
	if err := defaultResolver.SetResolverState(&wasm.SetResolverStateRequest{
		State:     initialState,
		AccountId: accountId,
	}); err != nil {
		t.Fatalf("Failed to initialize defaultResolver with state: %v", err)
	}

	// Update with new state - the key test is that UpdateStateAndFlushLogs succeeds
	newState := tu.LoadTestResolverState(t)
	err := defaultResolver.SetResolverState(&wasm.SetResolverStateRequest{
		State:     newState,
		AccountId: accountId,
	})
	if err != nil {
		t.Fatalf("UpdateStateAndFlushLogs failed: %v", err)
	}

	// Verify that we can successfully resolve after the state update
	request := tu.CreateResolveProcessRequest(tu.CreateTutorialFeatureRequest())

	processResponse, err := defaultResolver.ResolveProcess(request)
	if err != nil {
		t.Fatalf("Resolve failed after update: %v", err)
	}

	response := processResponse.GetResolved().GetResponse()
	if response == nil {
		t.Fatal("Expected successful resolve response")
	}

	// Verify we got the expected resolution
	if len(response.ResolvedFlags) != 1 {
		t.Errorf("Expected 1 resolved flag, got %d", len(response.ResolvedFlags))
	}

	if response.ResolvedFlags[0].Variant != "flags/tutorial-feature/variants/exciting-welcome" {
		t.Errorf("Expected exciting-welcome variant, got %s", response.ResolvedFlags[0].Variant)
	}

	t.Logf("✓ State update successful and flag resolution works correctly")
}

func TestSwapWasmResolverApi_MultipleUpdates(t *testing.T) {
	ctx := context.Background()

	// Load real test state
	initialState := tu.LoadTestResolverState(t)
	accountId := tu.LoadTestAccountID(t)

	defaultResolver := resolverFactory.New()
	defer defaultResolver.Close(ctx)

	// Initialize with test state
	if err := defaultResolver.SetResolverState(&wasm.SetResolverStateRequest{
		State:     initialState,
		AccountId: accountId,
	}); err != nil {
		t.Fatalf("Failed to initialize defaultResolver with state: %v", err)
	}

	// Perform multiple state updates to verify the defaultResolver mechanism works correctly
	for i := 0; i < 3; i++ {
		newState := tu.LoadTestResolverState(t)
		err := defaultResolver.SetResolverState(&wasm.SetResolverStateRequest{
			State:     newState,
			AccountId: accountId,
		})
		if err != nil {
			t.Fatalf("Update %d failed: %v", i, err)
		}

		// Verify that Resolve successfully works after each update
		request := tu.CreateResolveProcessRequest(tu.CreateTutorialFeatureRequest())

		processResponse, resolveErr := defaultResolver.ResolveProcess(request)
		if resolveErr != nil {
			t.Fatalf("Update %d: Resolve failed: %v", i, resolveErr)
		}

		response := processResponse.GetResolved().GetResponse()
		if response == nil {
			t.Fatalf("Update %d: Expected successful resolve response", i)
		}

		// Verify we got the expected variant after each defaultResolver
		if len(response.ResolvedFlags) != 1 {
			t.Errorf("Update %d: Expected 1 resolved flag, got %d", i, len(response.ResolvedFlags))
		} else if response.ResolvedFlags[0].Variant != "flags/tutorial-feature/variants/exciting-welcome" {
			t.Errorf("Update %d: Expected exciting-welcome variant, got %s", i, response.ResolvedFlags[0].Variant)
		}

		t.Logf("Update %d: ✓ Swap successful, flag resolves correctly", i)
	}
}

func TestSwapWasmResolverApi_Close(t *testing.T) {

	initialState := tu.CreateMinimalResolverState()
	accountId := "test-account"

	defaultResolver := resolverFactory.New()

	// Initialize with test state
	if err := defaultResolver.SetResolverState(&wasm.SetResolverStateRequest{
		State:     initialState,
		AccountId: accountId,
	}); err != nil {
		t.Fatalf("Failed to initialize defaultResolver with state: %v", err)
	}

	// Close should not panic
	// defaultResolver.Close(ctx)

	// Note: Closing again may cause issues with WASM module, so we don't test double-close
}

// func TestErrInstanceClosed(t *testing.T) {
// 	err := ErrInstanceClosed
// 	if err.Error() != "WASM instance is closed or being replaced" {
// 		t.Errorf("Unexpected error message: %s", err.Error())
// 	}

// 	// Test that errors.Is works with it
// 	testErr := ErrInstanceClosed
// 	if !errors.Is(testErr, ErrInstanceClosed) {
// 		t.Error("Expected errors.Is to work with ErrInstanceClosed")
// 	}
// }

// State from data sample, flag without sticky rules
func TestSwapWasmResolverApi_ResolveFlagWithNoStickyRules(t *testing.T) {
	ctx := context.Background()

	testState := tu.LoadTestResolverState(t)
	testAcctID := tu.LoadTestAccountID(t)

	defaultResolver := resolverFactory.New()
	defer defaultResolver.Close(ctx)

	// Initialize with test state
	if err := defaultResolver.SetResolverState(&wasm.SetResolverStateRequest{
		State:     testState,
		AccountId: testAcctID,
	}); err != nil {
		t.Fatalf("Failed to initialize defaultResolver with state: %v", err)
	}

	processRequest := tu.CreateResolveProcessRequest(tu.CreateTutorialFeatureRequest())

	response, err := defaultResolver.ResolveProcess(processRequest)
	if err != nil {
		t.Fatalf("Unexpected error resolving tutorial-feature flag: %v", err)
	}

	if response == nil {
		t.Fatal("Expected non-nil response")
	}

	resolvedResult, ok := response.Result.(*wasm.ResolveProcessResponse_Resolved_)
	if !ok {
		t.Fatal("Expected Resolved result from ResolveProcess")
	}

	resolveResponse := resolvedResult.Resolved.Response
	if len(resolveResponse.ResolvedFlags) != 1 {
		t.Fatalf("Expected 1 resolved flag, got %d", len(resolveResponse.ResolvedFlags))
	}

	resolvedFlag := resolveResponse.ResolvedFlags[0]

	if resolvedFlag.Flag != "flags/tutorial-feature" {
		t.Errorf("Expected flag 'flags/tutorial-feature', got '%s'", resolvedFlag.Flag)
	}

	expectedVariant := "flags/tutorial-feature/variants/exciting-welcome"
	if resolvedFlag.Variant != expectedVariant {
		t.Errorf("Expected variant '%s', got '%s'", expectedVariant, resolvedFlag.Variant)
	}

	if resolvedFlag.Reason.String() != "RESOLVE_REASON_MATCH" {
		t.Errorf("Expected reason RESOLVE_REASON_MATCH, got %v", resolvedFlag.Reason)
	}

	if resolvedFlag.Value == nil {
		t.Fatal("Expected non-nil value in resolved flag")
	}

	fields := resolvedFlag.Value.GetFields()
	if fields == nil {
		t.Fatal("Expected fields in resolved value")
	}

	expectedMessage := "We are very excited to welcome you to Confidence! This is a message from the tutorial flag."
	messageValue, hasMessage := fields["message"]
	if !hasMessage {
		t.Error("Expected 'message' field in resolved value")
	} else if messageValue.GetStringValue() != expectedMessage {
		t.Errorf("Expected message '%s', got '%s'", expectedMessage, messageValue.GetStringValue())
	}

	expectedTitle := "Welcome to Confidence!"
	titleValue, hasTitle := fields["title"]
	if !hasTitle {
		t.Error("Expected 'title' field in resolved value")
	} else if titleValue.GetStringValue() != expectedTitle {
		t.Errorf("Expected title '%s', got '%s'", expectedTitle, titleValue.GetStringValue())
	}
}

func TestSwapWasmResolverApi_ResolveFlagWithStickyRules_MissingMaterializations(t *testing.T) {
	ctx := context.Background()

	stickyState := tu.CreateStateWithStickyFlag()
	accountId := "test-account"

	defaultResolver := resolverFactory.New()
	defer defaultResolver.Close(ctx)

	// Initialize with test state
	if err := defaultResolver.SetResolverState(&wasm.SetResolverStateRequest{
		State:     stickyState,
		AccountId: accountId,
	}); err != nil {
		t.Fatalf("Failed to initialize defaultResolver with state: %v", err)
	}

	// Simple Resolve (no materializations) — should suspend
	processRequest := tu.CreateResolveProcessRequest(
		&resolver.ResolveFlagsRequest{
			Flags:        []string{"flags/sticky-test-flag"},
			Apply:        true,
			ClientSecret: "test-secret",
			EvaluationContext: &structpb.Struct{
				Fields: map[string]*structpb.Value{
					"user_id": structpb.NewStringValue("test-user-123"),
				},
			},
		},
	)

	response, err := defaultResolver.ResolveProcess(processRequest)
	if err != nil {
		t.Fatalf("Unexpected error from ResolveProcess: %v", err)
	}

	if response == nil {
		t.Fatal("Expected non-nil response")
	}

	// The response should be Suspended, not Resolved
	suspendedResult, ok := response.Result.(*wasm.ResolveProcessResponse_Suspended_)
	if !ok {
		t.Fatal("Expected Suspended result, got Resolved or other type")
	}

	if suspendedResult.Suspended == nil {
		t.Fatal("Expected non-nil Suspended")
	}
	if len(suspendedResult.Suspended.MaterializationsToRead) == 0 {
		t.Fatal("Expected non-empty materializations_to_read")
	}
}
