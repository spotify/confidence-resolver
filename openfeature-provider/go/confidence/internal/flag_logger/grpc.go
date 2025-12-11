package flag_logger

import (
	"context"
	"fmt"
	"log/slog"
	"sync"
	"time"

	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	"google.golang.org/grpc/metadata"
)

type GrpcFlagLogger struct {
	stub         resolverv1.InternalFlagLoggerServiceClient
	clientSecret string
	logger       *slog.Logger
	wg           sync.WaitGroup
}

func NewGrpcWasmFlagLogger(stub resolverv1.InternalFlagLoggerServiceClient, clientSecret string, logger *slog.Logger) *GrpcFlagLogger {
	return &GrpcFlagLogger{
		stub:         stub,
		clientSecret: clientSecret,
		logger:       logger,
	}
}

// Write writes flag logs, splitting into chunks if necessary
func (g *GrpcFlagLogger) Write(request *resolverv1.WriteFlagLogsRequest) {
	flagAssignedCount := len(request.FlagAssigned)
	clientResolveCount := len(request.ClientResolveInfo)
	flagResolveCount := len(request.FlagResolveInfo)

	if clientResolveCount == 0 && flagAssignedCount == 0 && flagResolveCount == 0 {
		g.logger.Debug("Skipping empty flag log request")
		return
	}

	if request.TelemetryData != nil {
		sdkID := "nil"
		sdkVersion := "nil"
		if request.TelemetryData.Sdk != nil {
			sdkID = request.TelemetryData.Sdk.GetId().String()
			sdkVersion = request.TelemetryData.Sdk.Version
		}
		g.logger.Debug("Telemetry Data",
			"sdk_id", sdkID,
			"sdk_version", sdkVersion)
	}

	g.logger.Debug("Sending flag logs",
		"flag_assigned", flagAssignedCount,
		"client_resolve_info", clientResolveCount,
		"flag_resolve_info", flagResolveCount)

	g.sendAsync(request)

}

func (g *GrpcFlagLogger) sendAsync(request *resolverv1.WriteFlagLogsRequest) {
	g.wg.Add(1)
	go func() {
		defer g.wg.Done()
		// Create a context with timeout for the RPC
		rpcCtx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()

		// Add Authorization header with client secret
		md := metadata.Pairs("authorization", fmt.Sprintf("ClientSecret %s", g.clientSecret))
		rpcCtx = metadata.NewOutgoingContext(rpcCtx, md)

		if _, err := g.stub.ClientWriteFlagLogs(rpcCtx, request); err != nil {
			g.logger.Error("Failed to write flag logs", "error", err)
		} else {
			g.logger.Debug("Successfully sent flag log", "entries", len(request.FlagAssigned))
		}
	}()
}

// Shutdown waits for all pending async writes to complete
func (g *GrpcFlagLogger) Shutdown() {
	g.wg.Wait()
}

// NoOpWasmFlagLogger is a flag logger that drops all requests (for disabled logging)
type NoOpWasmFlagLogger struct{}

func NewNoOpWasmFlagLogger() *NoOpWasmFlagLogger {
	return &NoOpWasmFlagLogger{}
}

func (n *NoOpWasmFlagLogger) Write(request *resolverv1.WriteFlagLogsRequest) {
	// Drop the request - do nothing
}

func (n *NoOpWasmFlagLogger) Shutdown() {
	// Nothing to shut down
}
