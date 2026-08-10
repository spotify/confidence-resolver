package flag_logger

import (
	"bytes"
	"context"
	"errors"
	"log/slog"
	"strings"
	"sync/atomic"
	"testing"

	admin "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/admin"
	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	"google.golang.org/grpc"
)

func TestMultiDestinationFlagLogger_DefaultsToEdge(t *testing.T) {
	var grpcCalled atomic.Int32
	mockStub := &mockInternalFlagLoggerServiceClient{
		writeFlagLogsFunc: func(ctx context.Context, req *resolverv1.WriteFlagLogsRequest) (*resolverv1.WriteFlagLogsResponse, error) {
			grpcCalled.Add(1)
			return &resolverv1.WriteFlagLogsResponse{}, nil
		},
	}

	logger := NewMultiDestinationFlagLogger(
		mockStub,
		"test-secret",
		func() []admin.LogDestination { return nil }, // empty destinations
		func() string { return "test-account" },
		slog.New(slog.NewTextHandler(&bytes.Buffer{}, nil)),
	)

	logger.Write(&resolverv1.WriteFlagLogsRequest{
		FlagAssigned: make([]*resolverv1.FlagAssigned, 1),
	})
	logger.Shutdown()

	if grpcCalled.Load() != 1 {
		t.Errorf("Expected gRPC to be called once (default), got %d", grpcCalled.Load())
	}
}

func TestMultiDestinationFlagLogger_RoutesToEdge(t *testing.T) {
	var grpcCalled atomic.Int32
	mockStub := &mockInternalFlagLoggerServiceClient{
		writeFlagLogsFunc: func(ctx context.Context, req *resolverv1.WriteFlagLogsRequest) (*resolverv1.WriteFlagLogsResponse, error) {
			grpcCalled.Add(1)
			return &resolverv1.WriteFlagLogsResponse{}, nil
		},
	}

	logger := NewMultiDestinationFlagLogger(
		mockStub,
		"test-secret",
		func() []admin.LogDestination {
			return []admin.LogDestination{admin.LogDestination_LOG_DESTINATION_SPOTIFY_EDGE}
		},
		func() string { return "test-account" },
		slog.New(slog.NewTextHandler(&bytes.Buffer{}, nil)),
	)

	logger.Write(&resolverv1.WriteFlagLogsRequest{
		FlagAssigned: make([]*resolverv1.FlagAssigned, 1),
	})
	logger.Shutdown()

	if grpcCalled.Load() != 1 {
		t.Errorf("Expected gRPC to be called once, got %d", grpcCalled.Load())
	}
}

func TestMultiDestinationFlagLogger_FallbackOnPrimaryFailure(t *testing.T) {
	// Primary is gRPC, fallback is Cloudflare. gRPC fails -> Cloudflare should be tried.
	// We can't easily test the HTTP path end-to-end, so we override the senders map.

	var primaryCalled, fallbackCalled atomic.Int32

	logger := &MultiDestinationFlagLogger{
		senders: map[admin.LogDestination]logSender{
			admin.LogDestination_LOG_DESTINATION_SPOTIFY_EDGE: func(ctx context.Context, req *resolverv1.WriteFlagLogsRequest) error {
				primaryCalled.Add(1)
				return errors.New("primary failed")
			},
			admin.LogDestination_LOG_DESTINATION_CLOUDFLARE: func(ctx context.Context, req *resolverv1.WriteFlagLogsRequest) error {
				fallbackCalled.Add(1)
				return nil
			},
		},
		destinations: func() []admin.LogDestination {
			return []admin.LogDestination{
				admin.LogDestination_LOG_DESTINATION_SPOTIFY_EDGE,
				admin.LogDestination_LOG_DESTINATION_CLOUDFLARE,
			}
		},
		logger: slog.New(slog.NewTextHandler(&bytes.Buffer{}, nil)),
	}

	logger.Write(&resolverv1.WriteFlagLogsRequest{
		FlagAssigned: make([]*resolverv1.FlagAssigned, 1),
	})
	logger.Shutdown()

	if primaryCalled.Load() != 1 {
		t.Errorf("Expected primary to be called once, got %d", primaryCalled.Load())
	}
	if fallbackCalled.Load() != 1 {
		t.Errorf("Expected fallback to be called once, got %d", fallbackCalled.Load())
	}
}

func TestMultiDestinationFlagLogger_NoPrimarySuccess_NoFallback(t *testing.T) {
	var primaryCalled, fallbackCalled atomic.Int32

	logger := &MultiDestinationFlagLogger{
		senders: map[admin.LogDestination]logSender{
			admin.LogDestination_LOG_DESTINATION_SPOTIFY_EDGE: func(ctx context.Context, req *resolverv1.WriteFlagLogsRequest) error {
				primaryCalled.Add(1)
				return nil // success
			},
			admin.LogDestination_LOG_DESTINATION_CLOUDFLARE: func(ctx context.Context, req *resolverv1.WriteFlagLogsRequest) error {
				fallbackCalled.Add(1)
				return nil
			},
		},
		destinations: func() []admin.LogDestination {
			return []admin.LogDestination{
				admin.LogDestination_LOG_DESTINATION_SPOTIFY_EDGE,
				admin.LogDestination_LOG_DESTINATION_CLOUDFLARE,
			}
		},
		logger: slog.New(slog.NewTextHandler(&bytes.Buffer{}, nil)),
	}

	logger.Write(&resolverv1.WriteFlagLogsRequest{
		FlagAssigned: make([]*resolverv1.FlagAssigned, 1),
	})
	logger.Shutdown()

	if primaryCalled.Load() != 1 {
		t.Errorf("Expected primary to be called once, got %d", primaryCalled.Load())
	}
	if fallbackCalled.Load() != 0 {
		t.Errorf("Expected fallback NOT to be called, got %d", fallbackCalled.Load())
	}
}

func TestMultiDestinationFlagLogger_SkipsEmptyRequest(t *testing.T) {
	var called atomic.Int32

	mockStub := &mockInternalFlagLoggerServiceClient{
		writeFlagLogsFunc: func(ctx context.Context, req *resolverv1.WriteFlagLogsRequest) (*resolverv1.WriteFlagLogsResponse, error) {
			called.Add(1)
			return &resolverv1.WriteFlagLogsResponse{}, nil
		},
	}

	logger := NewMultiDestinationFlagLogger(
		mockStub,
		"test-secret",
		func() []admin.LogDestination {
			return []admin.LogDestination{admin.LogDestination_LOG_DESTINATION_SPOTIFY_EDGE}
		},
		func() string { return "test-account" },
		slog.New(slog.NewTextHandler(&bytes.Buffer{}, nil)),
	)

	logger.Write(&resolverv1.WriteFlagLogsRequest{}) // empty
	logger.Shutdown()

	if called.Load() != 0 {
		t.Errorf("Expected no calls for empty request, got %d", called.Load())
	}
}

func TestMultiDestinationFlagLogger_AllDestinationsFail(t *testing.T) {
	var buf bytes.Buffer
	testLogger := slog.New(slog.NewTextHandler(&buf, &slog.HandlerOptions{Level: slog.LevelWarn}))

	logger := &MultiDestinationFlagLogger{
		senders: map[admin.LogDestination]logSender{
			admin.LogDestination_LOG_DESTINATION_SPOTIFY_EDGE: func(ctx context.Context, req *resolverv1.WriteFlagLogsRequest) error {
				return errors.New("edge failed")
			},
			admin.LogDestination_LOG_DESTINATION_CLOUDFLARE: func(ctx context.Context, req *resolverv1.WriteFlagLogsRequest) error {
				return errors.New("cloudflare failed")
			},
		},
		destinations: func() []admin.LogDestination {
			return []admin.LogDestination{
				admin.LogDestination_LOG_DESTINATION_SPOTIFY_EDGE,
				admin.LogDestination_LOG_DESTINATION_CLOUDFLARE,
			}
		},
		logger: testLogger,
	}

	// Send 10 requests to hit the failure stats window
	for i := 0; i < 10; i++ {
		logger.Write(&resolverv1.WriteFlagLogsRequest{
			FlagAssigned: make([]*resolverv1.FlagAssigned, 1),
		})
	}
	logger.Shutdown()

	output := buf.String()
	if !strings.Contains(output, "Flag log write failures") {
		t.Error("Expected failure log after window with all failures")
	}
}

func TestMultiDestinationFlagLogger_CloudflarePrimary(t *testing.T) {
	var cfCalled, grpcCalled atomic.Int32

	logger := &MultiDestinationFlagLogger{
		senders: map[admin.LogDestination]logSender{
			admin.LogDestination_LOG_DESTINATION_SPOTIFY_EDGE: func(ctx context.Context, req *resolverv1.WriteFlagLogsRequest) error {
				grpcCalled.Add(1)
				return nil
			},
			admin.LogDestination_LOG_DESTINATION_CLOUDFLARE: func(ctx context.Context, req *resolverv1.WriteFlagLogsRequest) error {
				cfCalled.Add(1)
				return nil
			},
		},
		destinations: func() []admin.LogDestination {
			return []admin.LogDestination{
				admin.LogDestination_LOG_DESTINATION_CLOUDFLARE,
				admin.LogDestination_LOG_DESTINATION_SPOTIFY_EDGE,
			}
		},
		logger: slog.New(slog.NewTextHandler(&bytes.Buffer{}, nil)),
	}

	logger.Write(&resolverv1.WriteFlagLogsRequest{
		FlagAssigned: make([]*resolverv1.FlagAssigned, 1),
	})
	logger.Shutdown()

	if cfCalled.Load() != 1 {
		t.Errorf("Expected Cloudflare (primary) to be called once, got %d", cfCalled.Load())
	}
	if grpcCalled.Load() != 0 {
		t.Errorf("Expected gRPC (fallback) NOT to be called on primary success, got %d", grpcCalled.Load())
	}
}

func TestMultiDestinationFlagLogger_GrpcSenderUsesClientWriteFlagLogs(t *testing.T) {
	var methodCalled atomic.Int32
	mockStub := &mockInternalFlagLoggerServiceClient{
		writeFlagLogsFunc: func(ctx context.Context, req *resolverv1.WriteFlagLogsRequest) (*resolverv1.WriteFlagLogsResponse, error) {
			methodCalled.Add(1)
			return &resolverv1.WriteFlagLogsResponse{}, nil
		},
	}

	sender := makeGrpcSender(mockStub, "test-secret")
	err := sender(context.Background(), &resolverv1.WriteFlagLogsRequest{
		FlagAssigned: make([]*resolverv1.FlagAssigned, 1),
	})

	if err != nil {
		t.Errorf("Expected no error, got %v", err)
	}
	if methodCalled.Load() != 1 {
		t.Errorf("Expected ClientWriteFlagLogs to be called once, got %d", methodCalled.Load())
	}
}

// Extend the mock to also handle the interface used by NewMultiDestinationFlagLogger
func (m *mockInternalFlagLoggerServiceClient) WriteMaterializedOperations(ctx context.Context, req *resolverv1.WriteOperationsRequest, opts ...grpc.CallOption) (*resolverv1.WriteOperationsResult, error) {
	return &resolverv1.WriteOperationsResult{}, nil
}

func (m *mockInternalFlagLoggerServiceClient) ReadMaterializedOperations(ctx context.Context, req *resolverv1.ReadOperationsRequest, opts ...grpc.CallOption) (*resolverv1.ReadOperationsResult, error) {
	return &resolverv1.ReadOperationsResult{}, nil
}
