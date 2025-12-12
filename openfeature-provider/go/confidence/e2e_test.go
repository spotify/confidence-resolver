package confidence

import (
	"context"
	"testing"

	"github.com/open-feature/go-sdk/openfeature"
)

// End-to-end tests that verify WriteFlagLogs successfully sends to the real backend.

const (
	e2eClientSecret         = "Ip7lGcBeGA4Le9MI8md4i5LkUOnLnyFx"
	e2eIncludedTargetingKey = "user-a"
	e2eExcludedTargetingKey = "user-x"
)

func TestFlagResolve_WithMaterializedSegmentTargetingAndRemoteMaterializationStore(t *testing.T) {
	ctx := context.Background()

	// Create a real provider with a RemoteMaterializationStore
	provider, err := NewProvider(ctx, ProviderConfig{
		ClientSecret:                  e2eClientSecret,
		UseRemoteMaterializationStore: true,
	})
	if err != nil {
		t.Fatalf("Failed to create provider: %v", err)
	}

	// Set provider and wait for ready
	err = openfeature.SetProviderAndWait(provider)
	if err != nil {
		t.Fatalf("Failed to set provider: %v", err)
	}

	client := openfeature.NewClient("real-backend-e2e-test")

	// Perform a resolve to generate logs
	details, err := client.StringValueDetails(ctx, "custom-targeted-flag.message", "client default", openfeature.NewTargetlessEvaluationContext(map[string]interface{}{
		"user_id": e2eIncludedTargetingKey,
	}))
	if err != nil {
		t.Fatalf("Failed to resolve string flag: %v", err)
	}
	if details.Variant != "flags/custom-targeted-flag/variants/cake-exclamation" {
		t.Errorf("Expected Variant flags/custom-targeted-flag/variants/cake-exclamation, got %v", details.Variant)
	}

	openfeature.Shutdown()
}

func TestFlagResolve_WithMaterializedSegmentTargetingAndRemoteMaterializationStoreNotInSegment(t *testing.T) {
	ctx := context.Background()

	// Create a real provider with a RemoteMaterializationStore
	provider, err := NewProvider(ctx, ProviderConfig{
		ClientSecret:                  e2eClientSecret,
		UseRemoteMaterializationStore: true,
	})
	if err != nil {
		t.Fatalf("Failed to create provider: %v", err)
	}

	// Set provider and wait for ready
	err = openfeature.SetProviderAndWait(provider)
	if err != nil {
		t.Fatalf("Failed to set provider: %v", err)
	}

	client := openfeature.NewClient("real-backend-e2e-test")

	// Perform a resolve to generate logs
	details, err := client.StringValueDetails(ctx, "custom-targeted-flag.message", "client default", openfeature.NewTargetlessEvaluationContext(map[string]interface{}{
		"user_id": e2eExcludedTargetingKey,
	}))
	if err != nil {
		t.Fatalf("Failed to resolve string flag: %v", err)
	}
	if details.Variant != "flags/custom-targeted-flag/variants/default" {
		t.Errorf("Expected Variant flags/custom-targeted-flag/variants/default, got %v", details.Variant)
	}

	openfeature.Shutdown()
}

func TestFlagResolve_WithMaterializedSegmentTargetingAndNoMaterializationStoreWillNotWork(t *testing.T) {
	ctx := context.Background()

	// Create a real provider with a RemoteMaterializationStore
	provider, err := NewProvider(ctx, ProviderConfig{
		ClientSecret:                  e2eClientSecret,
		UseRemoteMaterializationStore: false,
	})
	if err != nil {
		t.Fatalf("Failed to create provider: %v", err)
	}

	// Set provider and wait for ready
	err = openfeature.SetProviderAndWait(provider)
	if err != nil {
		t.Fatalf("Failed to set provider: %v", err)
	}

	client := openfeature.NewClient("real-backend-e2e-test")

	// Perform a resolve to generate logs
	details, err := client.StringValueDetails(ctx, "custom-targeted-flag.message", "client default", openfeature.NewTargetlessEvaluationContext(map[string]interface{}{
		"user_id": e2eIncludedTargetingKey,
	}))
	if err == nil {
		t.Fatalf("Expected to fail resolve got: %v", details)
	}
	if err.Error() != "error code: GENERAL: resolve failed: failed to handle missing materializations: materialization read not supported" {
		t.Fatalf("Expected different error message, got: %v", err)
	}
	if details.Reason != "ERROR" {
		t.Errorf("Expected Variant flags/custom-targeted-flag/variants/default, got %v", details.Reason)
	}

	if details.Value != "client default" {
		t.Errorf("Expected default value client default, got %v", details.Value)
	}

	openfeature.Shutdown()
}
