package confidence_test

import (
	"context"
	"log/slog"
	"net"
	"os"
	"sync/atomic"
	"testing"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence"
	fl "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/flag_logger"
	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/grpc/status"
	"google.golang.org/grpc/test/bufconn"
)

func TestRetryOnUnavailable(t *testing.T) {
	var callCount atomic.Int32
	srv := grpc.NewServer()
	resolverv1.RegisterInternalFlagLoggerServiceServer(srv, &retryTestService{
		callCount: &callCount,
		failUntil: 1,
	})

	lis := bufconn.Listen(1024 * 1024)
	go srv.Serve(lis)
	t.Cleanup(srv.Stop)

	conn, err := grpc.NewClient("passthrough:///bufconn",
		grpc.WithContextDialer(func(ctx context.Context, _ string) (net.Conn, error) {
			return lis.DialContext(ctx)
		}),
		grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithDefaultServiceConfig(confidence.RetryServiceConfig),
	)
	if err != nil {
		t.Fatalf("Failed to dial: %v", err)
	}
	t.Cleanup(func() { conn.Close() })

	stub := resolverv1.NewInternalFlagLoggerServiceClient(conn)
	logger := fl.NewGrpcWasmFlagLogger(stub, "test-secret", slog.New(slog.NewTextHandler(os.Stderr, nil)))

	logger.Write(&resolverv1.WriteFlagLogsRequest{
		FlagAssigned: []*resolverv1.FlagAssigned{{ResolveId: "r1"}},
	})
	logger.Shutdown()

	if got := callCount.Load(); got != 2 {
		t.Errorf("Expected 2 calls (1 fail + 1 retry), got %d", got)
	}
}

func TestNoRetryOnPermissionDenied(t *testing.T) {
	var callCount atomic.Int32
	srv := grpc.NewServer()
	resolverv1.RegisterInternalFlagLoggerServiceServer(srv, &permDeniedTestService{
		callCount: &callCount,
	})

	lis := bufconn.Listen(1024 * 1024)
	go srv.Serve(lis)
	t.Cleanup(srv.Stop)

	conn, err := grpc.NewClient("passthrough:///bufconn",
		grpc.WithContextDialer(func(ctx context.Context, _ string) (net.Conn, error) {
			return lis.DialContext(ctx)
		}),
		grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithDefaultServiceConfig(confidence.RetryServiceConfig),
	)
	if err != nil {
		t.Fatalf("Failed to dial: %v", err)
	}
	t.Cleanup(func() { conn.Close() })

	stub := resolverv1.NewInternalFlagLoggerServiceClient(conn)
	logger := fl.NewGrpcWasmFlagLogger(stub, "test-secret", slog.New(slog.NewTextHandler(os.Stderr, nil)))

	logger.Write(&resolverv1.WriteFlagLogsRequest{
		FlagAssigned: []*resolverv1.FlagAssigned{{ResolveId: "r1"}},
	})
	logger.Shutdown()

	if got := callCount.Load(); got != 1 {
		t.Errorf("Expected 1 call (no retry on PERMISSION_DENIED), got %d", got)
	}
}

type retryTestService struct {
	resolverv1.UnimplementedInternalFlagLoggerServiceServer
	callCount *atomic.Int32
	failUntil int32
}

func (s *retryTestService) ClientWriteFlagLogs(_ context.Context, _ *resolverv1.WriteFlagLogsRequest) (*resolverv1.WriteFlagLogsResponse, error) {
	n := s.callCount.Add(1)
	if n <= s.failUntil {
		return nil, status.Error(codes.Unavailable, "simulated")
	}
	return &resolverv1.WriteFlagLogsResponse{}, nil
}

type permDeniedTestService struct {
	resolverv1.UnimplementedInternalFlagLoggerServiceServer
	callCount *atomic.Int32
}

func (s *permDeniedTestService) ClientWriteFlagLogs(_ context.Context, _ *resolverv1.WriteFlagLogsRequest) (*resolverv1.WriteFlagLogsResponse, error) {
	s.callCount.Add(1)
	return nil, status.Error(codes.PermissionDenied, "not retryable")
}
