package confidence

import (
	"context"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
)

// End-to-end tests that verify WriteFlagLogs successfully sends to the real backend.

var flagLogsClientSecret = os.Getenv("CONFIDENCE_CLIENT_SECRET")

const flagLogsTargetingKey = "test-a"

// TestFlagLogs_ShouldSuccessfullySendToRealBackend tests that we can successfully
// send WriteFlagLogs to the real Confidence backend and get a successful response.
// This uses the real gRPC connection to edge-grpc.spotify.com.
func TestFlagLogs_ShouldSuccessfullySendToRealBackend(t *testing.T) {
	ctx := context.Background()

	// Create a custom logger that captures log messages
	logger := newRecorderLogger()

	// Create a real provider with real gRPC connection
	provider, err := NewProvider(ctx, ProviderConfig{
		ClientSecret: flagLogsClientSecret,
		Logger:       logger,
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
	value, err := client.BooleanValue(ctx, "web-sdk-e2e-flag.bool", true, openfeature.NewEvaluationContext(flagLogsTargetingKey, map[string]interface{}{
		"sticky": false,
	}))
	if err != nil {
		t.Fatalf("Failed to resolve boolean flag: %v", err)
	}
	if value != false {
		t.Errorf("Expected false, got %v", value)
	}

	// Shutdown - this flushes logs to the real backend via gRPC
	openfeature.Shutdown()

	// Give async operations time to complete
	time.Sleep(200 * time.Millisecond)

	// Verify the logs contain success message and no errors
	logs := logger.String()
	if strings.Contains(logs, "Failed to write flag logs") {
		t.Errorf("Backend returned error - found 'Failed to write flag logs' in logs:\n%s", logs)
	}
	if !strings.Contains(logs, "Successfully sent flag log") {
		t.Errorf("Expected 'Successfully sent flag log' in logs, but not found:\n%s", logs)
	}

	t.Log("Successfully sent WriteFlagLogs to real Confidence backend")
}
