package flag_logger

import (
	"context"
	"fmt"
	"log/slog"
	"sync"
	"sync/atomic"
	"time"

	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	"google.golang.org/grpc/metadata"
)

type GrpcFlagLogger struct {
	stub                  resolverv1.InternalFlagLoggerServiceClient
	clientSecret          string
	logger                *slog.Logger
	wg                    sync.WaitGroup
	attempts              atomic.Int64
	failures              atomic.Int64
	flushSucceeded        atomic.Int64
	flushFailed           atomic.Int64
	eventsPublished       atomic.Int64
	eventBatchesSucceeded atomic.Int64
	eventBatchesFailed    atomic.Int64
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

	if clientResolveCount == 0 && flagAssignedCount == 0 && flagResolveCount == 0 && request.TelemetryData == nil {
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

	succeeded := uint32(g.flushSucceeded.Swap(0))
	failed := uint32(g.flushFailed.Swap(0))
	evPub := uint32(g.eventsPublished.Swap(0))
	evOk := uint32(g.eventBatchesSucceeded.Swap(0))
	evFail := uint32(g.eventBatchesFailed.Swap(0))
	if succeeded > 0 || failed > 0 || evPub > 0 || evOk > 0 || evFail > 0 {
		if request.TelemetryData == nil {
			request.TelemetryData = &resolverv1.TelemetryData{}
		}
		if succeeded > 0 || failed > 0 {
			request.TelemetryData.Flush = &resolverv1.TelemetryData_FlushTelemetry{
				Succeeded: succeeded,
				Failed:    failed,
			}
		}
		if evPub > 0 || evOk > 0 || evFail > 0 {
			request.TelemetryData.Events = &resolverv1.TelemetryData_EventsTelemetry{
				Published:        evPub,
				BatchesSucceeded: evOk,
				BatchesFailed:    evFail,
			}
		}
	}

	g.sendAsync(request)

}

func (g *GrpcFlagLogger) RecordEventBatch(eventCount int, succeeded bool) {
	if succeeded {
		g.eventsPublished.Add(int64(eventCount))
		g.eventBatchesSucceeded.Add(1)
	} else {
		g.eventBatchesFailed.Add(1)
	}
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
			g.failures.Add(1)
			g.flushFailed.Add(1)
		} else {
			g.flushSucceeded.Add(1)
			g.logger.Debug("Successfully sent flag log",
				"flag_assigned", len(request.FlagAssigned),
				"client_resolve_info", len(request.ClientResolveInfo),
				"flag_resolve_info", len(request.FlagResolveInfo))
		}

		if g.attempts.Add(1)%10 == 0 {
			if failures := g.failures.Swap(0); failures > 0 {
				g.logger.Warn("Flag log write failures", "failures", failures, "window", 10)
			}
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

func (n *NoOpWasmFlagLogger) RecordEventBatch(eventCount int, succeeded bool) {
	// No-op
}
