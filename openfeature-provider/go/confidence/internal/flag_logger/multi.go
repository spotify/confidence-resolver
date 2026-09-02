package flag_logger

import (
	"context"
	"fmt"
	"log/slog"
	"sync"
	"sync/atomic"
	"time"

	admin "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/admin"
	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	"google.golang.org/grpc/metadata"
)

// logSender is a synchronous function that sends flag logs to a single destination.
type logSender func(ctx context.Context, request *resolverv1.WriteFlagLogsRequest) error

// MultiDestinationFlagLogger routes flag logs to destinations specified by the
// CDN state. The first destination is primary; the second is fallback on error.
// If no destinations are configured, it defaults to Spotify Edge (gRPC).
type MultiDestinationFlagLogger struct {
	senders               map[admin.LogDestination]logSender
	destinations          func() []admin.LogDestination
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

// NewMultiDestinationFlagLogger creates a flag logger that routes to multiple
// destinations based on the CDN state.
//
// grpcStub is the existing gRPC client for the Spotify Edge path.
// httpClient is used for the Cloudflare HTTP path (nil uses default).
// clientSecret is used for authorization on both paths.
// destinations returns the current ordered list of log destinations.
// accountID returns the current account ID (needed for the Cloudflare path).
func NewMultiDestinationFlagLogger(
	grpcStub resolverv1.InternalFlagLoggerServiceClient,
	clientSecret string,
	destinations func() []admin.LogDestination,
	accountID func() string,
	logger *slog.Logger,
) *MultiDestinationFlagLogger {
	httpS := newHttpSender(clientSecret, accountID, nil)

	senders := map[admin.LogDestination]logSender{
		admin.LogDestination_LOG_DESTINATION_SPOTIFY_EDGE: makeGrpcSender(grpcStub, clientSecret),
		admin.LogDestination_LOG_DESTINATION_CLOUDFLARE:   httpS.send,
	}

	return &MultiDestinationFlagLogger{
		senders:      senders,
		destinations: destinations,
		logger:       logger,
	}
}

// makeGrpcSender returns a logSender that sends via gRPC.
func makeGrpcSender(stub resolverv1.InternalFlagLoggerServiceClient, clientSecret string) logSender {
	return func(ctx context.Context, request *resolverv1.WriteFlagLogsRequest) error {
		md := metadata.Pairs("authorization", fmt.Sprintf("ClientSecret %s", clientSecret))
		rpcCtx := metadata.NewOutgoingContext(ctx, md)
		_, err := stub.ClientWriteFlagLogs(rpcCtx, request)
		return err
	}
}

// Write sends flag logs asynchronously, routing to the configured destinations.
func (m *MultiDestinationFlagLogger) Write(request *resolverv1.WriteFlagLogsRequest) {
	flagAssignedCount := len(request.FlagAssigned)
	clientResolveCount := len(request.ClientResolveInfo)
	flagResolveCount := len(request.FlagResolveInfo)

	if clientResolveCount == 0 && flagAssignedCount == 0 && flagResolveCount == 0 && request.TelemetryData == nil {
		m.logger.Debug("Skipping empty flag log request")
		return
	}

	m.logger.Debug("Sending flag logs",
		"flag_assigned", flagAssignedCount,
		"client_resolve_info", clientResolveCount,
		"flag_resolve_info", flagResolveCount)

	succeeded := uint32(m.flushSucceeded.Swap(0))
	failed := uint32(m.flushFailed.Swap(0))
	evPub := uint32(m.eventsPublished.Swap(0))
	evOk := uint32(m.eventBatchesSucceeded.Swap(0))
	evFail := uint32(m.eventBatchesFailed.Swap(0))
	if succeeded > 0 || failed > 0 || evPub > 0 || evOk > 0 || evFail > 0 {
		if request.TelemetryData == nil {
			request.TelemetryData = &resolverv1.TelemetryData{}
		}
		request.TelemetryData.FlushSucceeded = succeeded
		request.TelemetryData.FlushFailed = failed
		request.TelemetryData.EventsPublished = evPub
		request.TelemetryData.EventBatchesSucceeded = evOk
		request.TelemetryData.EventBatchesFailed = evFail
	}

	m.wg.Add(1)
	go func() {
		defer m.wg.Done()

		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()

		dests := m.resolveDestinations()
		var lastErr error

		for i, dest := range dests {
			sender, ok := m.senders[dest]
			if !ok {
				m.logger.Warn("Unknown log destination, skipping", "destination", dest)
				continue
			}

			if err := sender(ctx, request); err != nil {
				label := "primary"
				if i > 0 {
					label = "fallback"
				}
				m.logger.Warn("Flag log send failed", "destination", dest, "role", label, "error", err)
				lastErr = err
				continue // try next destination
			}

			// Success
			m.logger.Debug("Successfully sent flag log",
				"destination", dest,
				"flag_assigned", len(request.FlagAssigned),
				"client_resolve_info", len(request.ClientResolveInfo),
				"flag_resolve_info", len(request.FlagResolveInfo))
			lastErr = nil
			break
		}

		if lastErr != nil {
			m.failures.Add(1)
			m.flushFailed.Add(1)
		} else {
			m.flushSucceeded.Add(1)
		}

		if m.attempts.Add(1)%10 == 0 {
			if failures := m.failures.Swap(0); failures > 0 {
				m.logger.Warn("Flag log write failures", "failures", failures, "window", 10)
			}
		}
	}()
}

// resolveDestinations returns the ordered list of destinations to try.
// Falls back to Spotify Edge if none are configured.
func (m *MultiDestinationFlagLogger) resolveDestinations() []admin.LogDestination {
	dests := m.destinations()
	if len(dests) == 0 {
		return []admin.LogDestination{admin.LogDestination_LOG_DESTINATION_SPOTIFY_EDGE}
	}
	return dests
}

// Shutdown waits for all pending async writes to complete.
func (m *MultiDestinationFlagLogger) Shutdown() {
	m.wg.Wait()
}

func (m *MultiDestinationFlagLogger) RecordEventBatch(eventCount int, succeeded bool) {
	if succeeded {
		m.eventsPublished.Add(int64(eventCount))
		m.eventBatchesSucceeded.Add(1)
	} else {
		m.eventBatchesFailed.Add(1)
	}
}
