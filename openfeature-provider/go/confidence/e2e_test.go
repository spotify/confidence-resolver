package confidence

import (
	"context"
	"os"
	"testing"

	"github.com/open-feature/go-sdk/openfeature"
)

// End-to-end tests that verify WriteFlagLogs successfully sends to the real backend.

var e2eClientSecret = os.Getenv("CONFIDENCE_CLIENT_SECRET")
var e2eEncryptionKey = os.Getenv("CONFIDENCE_CLIENT_ENCRYPTION_KEY")

const (
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

func TestFlagResolve_WithMaterializedSegmentTargetingAndNoMaterializationStoreUsesBloomFilter(t *testing.T) {
	ctx := context.Background()

	// Without a materialization store, the resolver uses bloom filters
	// delivered in the state to check materialized segment membership locally.
	provider, err := NewProvider(ctx, ProviderConfig{
		ClientSecret:                  e2eClientSecret,
		UseRemoteMaterializationStore: false,
	})
	if err != nil {
		t.Fatalf("Failed to create provider: %v", err)
	}

	err = openfeature.SetProviderAndWait(provider)
	if err != nil {
		t.Fatalf("Failed to set provider: %v", err)
	}

	client := openfeature.NewClient("real-backend-e2e-test")

	details, err := client.StringValueDetails(ctx, "custom-targeted-flag.message", "client default", openfeature.NewTargetlessEvaluationContext(map[string]interface{}{
		"user_id": e2eIncludedTargetingKey,
	}))
	if err != nil {
		t.Fatalf("Failed to resolve string flag via bloom filter: %v", err)
	}
	if details.Variant != "flags/custom-targeted-flag/variants/cake-exclamation" {
		t.Errorf("Expected Variant flags/custom-targeted-flag/variants/cake-exclamation, got %v", details.Variant)
	}

	openfeature.Shutdown()
}

func TestFlagResolve_WithEncryptedState(t *testing.T) {
	ctx := context.Background()
	provider, err := NewProvider(ctx, ProviderConfig{
		ClientSecret:  e2eClientSecret,
		EncryptionKey: e2eEncryptionKey,
	})
	if err != nil {
		t.Fatalf("Failed to create provider: %v", err)
	}
	err = openfeature.SetProviderAndWait(provider)
	if err != nil {
		t.Fatalf("Failed to set provider: %v", err)
	}
	defer provider.Shutdown()
	client := openfeature.NewClient("encrypted-e2e")

	// Same flag assertions as existing e2e tests
	details, err := client.BooleanValueDetails(ctx, "web-sdk-e2e-flag.bool", true, openfeature.NewTargetlessEvaluationContext(map[string]interface{}{
		"targeting_key": "test-a",
		"sticky":        false,
	}))
	if err != nil {
		t.Fatalf("Failed to resolve: %v", err)
	}
	if details.Value != false {
		t.Errorf("Expected false, got %v", details.Value)
	}
}
