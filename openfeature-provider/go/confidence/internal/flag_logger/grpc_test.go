package flag_logger

import (
	"context"
	"errors"
	"log/slog"
	"os"
	"sync/atomic"
	"testing"
	"time"

	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/resolverinternal"
	"google.golang.org/grpc"
)

// mockInternalFlagLoggerServiceClient is a mock implementation for testing
type mockInternalFlagLoggerServiceClient struct {
	resolverv1.InternalFlagLoggerServiceClient
	writeFlagLogsFunc func(ctx context.Context, req *resolverv1.WriteFlagLogsRequest) (*resolverv1.WriteFlagLogsResponse, error)
}

func (m *mockInternalFlagLoggerServiceClient) WriteFlagLogs(ctx context.Context, req *resolverv1.WriteFlagLogsRequest, opts ...grpc.CallOption) (*resolverv1.WriteFlagLogsResponse, error) {
	if m.writeFlagLogsFunc != nil {
		return m.writeFlagLogsFunc(ctx, req)
	}
	return &resolverv1.WriteFlagLogsResponse{}, nil
}

func (m *mockInternalFlagLoggerServiceClient) ClientWriteFlagLogs(ctx context.Context, req *resolverv1.WriteFlagLogsRequest, opts ...grpc.CallOption) (*resolverv1.WriteFlagLogsResponse, error) {
	if m.writeFlagLogsFunc != nil {
		return m.writeFlagLogsFunc(ctx, req)
	}
	return &resolverv1.WriteFlagLogsResponse{}, nil
}

func TestNewGrpcWasmFlagLogger(t *testing.T) {
	mockStub := &mockInternalFlagLoggerServiceClient{}
	logger := NewGrpcWasmFlagLogger(mockStub, "test-client-secret", slog.New(slog.NewTextHandler(os.Stderr, nil)))

	if logger == nil {
		t.Fatal("Expected logger to be created, got nil")
	}
	if logger.stub == nil {
		t.Error("Expected stub to be set correctly")
	}
}

func TestGrpcWasmFlagLogger_Write_Empty(t *testing.T) {
	callCount := 0
	mockStub := &mockInternalFlagLoggerServiceClient{
		writeFlagLogsFunc: func(ctx context.Context, req *resolverv1.WriteFlagLogsRequest) (*resolverv1.WriteFlagLogsResponse, error) {
			callCount++
			return &resolverv1.WriteFlagLogsResponse{}, nil
		},
	}

	logger := NewGrpcWasmFlagLogger(mockStub, "test-client-secret", slog.New(slog.NewTextHandler(os.Stderr, nil)))

	// Empty request should be skipped
	request := &resolverv1.WriteFlagLogsRequest{}
	logger.Write(request)

	// Wait for async processing
	logger.Shutdown()

	if callCount != 0 {
		t.Errorf("Expected 0 calls for empty request, got %d", callCount)
	}
}

func TestGrpcWasmFlagLogger_Write_SmallRequest(t *testing.T) {
	var callCount int32
	var receivedRequests []*resolverv1.WriteFlagLogsRequest

	mockStub := &mockInternalFlagLoggerServiceClient{
		writeFlagLogsFunc: func(ctx context.Context, req *resolverv1.WriteFlagLogsRequest) (*resolverv1.WriteFlagLogsResponse, error) {
			atomic.AddInt32(&callCount, 1)
			receivedRequests = append(receivedRequests, req)
			return &resolverv1.WriteFlagLogsResponse{}, nil
		},
	}

	logger := NewGrpcWasmFlagLogger(mockStub, "test-client-secret", slog.New(slog.NewTextHandler(os.Stderr, nil)))

	// Create a small request (below chunk threshold)
	request := &resolverv1.WriteFlagLogsRequest{
		FlagAssigned: make([]*resolverv1.FlagAssigned, 100),
	}

	logger.Write(request)

	// Wait for async processing
	logger.Shutdown()

	if atomic.LoadInt32(&callCount) != 1 {
		t.Errorf("Expected 1 call for small request, got %d", callCount)
	}
	if len(receivedRequests) != 1 {
		t.Fatalf("Expected 1 received request, got %d", len(receivedRequests))
	}
	if len(receivedRequests[0].FlagAssigned) != 100 {
		t.Errorf("Expected 100 flag_assigned entries, got %d", len(receivedRequests[0].FlagAssigned))
	}
}

func TestGrpcWasmFlagLogger_ErrorHandling(t *testing.T) {
	var callCount int32
	expectedErr := errors.New("test error")

	mockStub := &mockInternalFlagLoggerServiceClient{
		writeFlagLogsFunc: func(ctx context.Context, req *resolverv1.WriteFlagLogsRequest) (*resolverv1.WriteFlagLogsResponse, error) {
			atomic.AddInt32(&callCount, 1)
			return nil, expectedErr
		},
	}

	logger := NewGrpcWasmFlagLogger(mockStub, "test-client-secret", slog.New(slog.NewTextHandler(os.Stderr, nil)))

	request := &resolverv1.WriteFlagLogsRequest{
		FlagAssigned: make([]*resolverv1.FlagAssigned, 10),
	}

	// Write should not return error (async)
	logger.Write(request)

	// Wait for async processing
	logger.Shutdown()

	// Should still have attempted the call
	if atomic.LoadInt32(&callCount) != 1 {
		t.Errorf("Expected 1 call attempt, got %d", callCount)
	}
}

func TestGrpcWasmFlagLogger_Shutdown(t *testing.T) {
	var processedCount int32
	mockStub := &mockInternalFlagLoggerServiceClient{
		writeFlagLogsFunc: func(ctx context.Context, req *resolverv1.WriteFlagLogsRequest) (*resolverv1.WriteFlagLogsResponse, error) {
			time.Sleep(50 * time.Millisecond) // Simulate slow processing
			atomic.AddInt32(&processedCount, 1)
			return &resolverv1.WriteFlagLogsResponse{}, nil
		},
	}

	logger := NewGrpcWasmFlagLogger(mockStub, "test-client-secret", slog.New(slog.NewTextHandler(os.Stderr, nil)))

	// Send multiple requests
	for i := 0; i < 5; i++ {
		request := &resolverv1.WriteFlagLogsRequest{
			FlagAssigned: make([]*resolverv1.FlagAssigned, 10),
		}
		logger.Write(request)
	}

	// Shutdown should wait for all to complete
	logger.Shutdown()

	if atomic.LoadInt32(&processedCount) != 5 {
		t.Errorf("Expected all 5 requests to be processed, got %d", processedCount)
	}
}

func TestNoOpWasmFlagLogger(t *testing.T) {
	logger := NewNoOpWasmFlagLogger()

	request := &resolverv1.WriteFlagLogsRequest{
		FlagAssigned: make([]*resolverv1.FlagAssigned, 100),
	}

	// Should not return error
	logger.Write(request)

	// Shutdown should not panic
	logger.Shutdown()
}
