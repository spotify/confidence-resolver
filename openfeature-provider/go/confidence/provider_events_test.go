package confidence

import (
	"context"
	"errors"
	"io"
	"log/slog"
	"testing"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/events"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/eventswasm"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
)

// fakeEventTracker is an in-memory stand-in for the WASM event tracker. Each
// FlushEvents call returns one event per remaining batch, so `batches` controls
// how many non-empty flushes the drain loop observes.
type fakeEventTracker struct {
	remainingBatches int
	flushCalls       int
	flushErr         error
}

func (f *fakeEventTracker) TrackEvent(*eventswasm.TrackEventRequest) error { return nil }

func (f *fakeEventTracker) FlushEvents() (*eventswasm.FlushEventsResponse, error) {
	f.flushCalls++
	if f.flushErr != nil {
		return nil, f.flushErr
	}
	if f.remainingBatches <= 0 {
		return &eventswasm.FlushEventsResponse{}, nil
	}
	f.remainingBatches--
	return &eventswasm.FlushEventsResponse{Events: []*events.Event{{EventDefinition: "eventDefinitions/test"}}}, nil
}

func (f *fakeEventTracker) Close() error { return nil }

// fakeEventsClient records publish calls and can fail every call, emulating an
// unreachable events service.
type fakeEventsClient struct {
	calls int
	err   error
}

func (c *fakeEventsClient) PublishEvents(
	_ context.Context,
	_ *events.PublishEventsRequest,
	_ ...grpc.CallOption,
) (*events.PublishEventsResponse, error) {
	c.calls++
	if c.err != nil {
		return nil, c.err
	}
	return &events.PublishEventsResponse{}, nil
}

func newDrainTestProvider(tracker eventTracking, client events.EventsServiceClient) *LocalResolverProvider {
	return &LocalResolverProvider{
		clientSecret: "test-secret",
		logger:       slog.New(slog.NewTextHandler(io.Discard, &slog.HandlerOptions{Level: slog.LevelError})),
		eventTracker: tracker,
		eventsClient: client,
	}
}

func TestDrainEvents_LoopsUntilBufferEmpty(t *testing.T) {
	tracker := &fakeEventTracker{remainingBatches: 3}
	client := &fakeEventsClient{}
	provider := newDrainTestProvider(tracker, client)

	provider.drainEvents(context.Background())

	// 3 non-empty flushes plus the empty flush that ends the loop.
	if tracker.flushCalls != 4 {
		t.Errorf("Expected 4 flush calls, got %d", tracker.flushCalls)
	}
	if client.calls != 3 {
		t.Errorf("Expected 3 publish calls, got %d", client.calls)
	}
}

func TestDrainEvents_BoundedWhenPublishAlwaysFails(t *testing.T) {
	// Never empties: without the bound this would loop forever.
	tracker := &fakeEventTracker{remainingBatches: maxDrainBatches * 10}
	client := &fakeEventsClient{err: errors.New("events service unreachable")}
	provider := newDrainTestProvider(tracker, client)

	provider.drainEvents(context.Background())

	if tracker.flushCalls != maxDrainBatches {
		t.Errorf("Expected drain to stop after %d flushes, got %d", maxDrainBatches, tracker.flushCalls)
	}
	if client.calls != maxDrainBatches {
		t.Errorf("Expected %d publish calls, got %d", maxDrainBatches, client.calls)
	}
	// maxDrainBatches is a multiple of the window, so all failures are reported
	// and the counter is left at zero.
	if got := provider.eventPublishFailures.Load(); got != 0 {
		t.Errorf("Expected failure counter to be drained by the reporting window, got %d", got)
	}
	if got := provider.eventPublishAttempts.Load(); got != int64(maxDrainBatches) {
		t.Errorf("Expected %d publish attempts, got %d", maxDrainBatches, got)
	}
}

func TestDrainEvents_StopsOnCancelledContext(t *testing.T) {
	tracker := &fakeEventTracker{remainingBatches: maxDrainBatches * 10}
	client := &fakeEventsClient{err: errors.New("events service unreachable")}
	provider := newDrainTestProvider(tracker, client)

	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	provider.drainEvents(ctx)

	if tracker.flushCalls != 1 {
		t.Errorf("Expected drain to stop after the first round on a cancelled context, got %d flushes", tracker.flushCalls)
	}
}

func TestDrainEvents_NoTrackerIsNoOp(t *testing.T) {
	provider := newDrainTestProvider(nil, &fakeEventsClient{})
	provider.eventTracker = nil

	provider.drainEvents(context.Background())

	if provider.eventPublishAttempts.Load() != 0 {
		t.Error("Expected no publish attempts without an event tracker")
	}
}

func TestFlushAndPublishEvents_ReportsFailuresPerWindow(t *testing.T) {
	tracker := &fakeEventTracker{remainingBatches: eventPublishLogWindow}
	client := &fakeEventsClient{err: errors.New("events service unreachable")}
	provider := newDrainTestProvider(tracker, client)

	for i := 0; i < eventPublishLogWindow-1; i++ {
		provider.flushAndPublishEvents(context.Background())
		if got := provider.eventPublishFailures.Load(); got != int64(i+1) {
			t.Fatalf("Expected %d accumulated failures, got %d", i+1, got)
		}
	}

	// The window boundary swaps the accumulated failures out for reporting.
	provider.flushAndPublishEvents(context.Background())
	if got := provider.eventPublishFailures.Load(); got != 0 {
		t.Errorf("Expected failures to be reset at the window boundary, got %d", got)
	}
}

// TestEventsRetryServiceConfig_IsAccepted guards the retry policy JSON: gRPC
// rejects a malformed default service config when the client is created.
// grpc.NewClient is lazy, so this needs no network access.
func TestEventsRetryServiceConfig_IsAccepted(t *testing.T) {
	conn, err := grpc.NewClient(
		"passthrough:///"+eventsGrpcTarget,
		grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithDefaultServiceConfig(eventsRetryServiceConfig),
	)
	if err != nil {
		t.Fatalf("Expected events retry service config to be accepted, got %v", err)
	}
	if err := conn.Close(); err != nil {
		t.Errorf("Failed to close connection: %v", err)
	}
}

func TestFlushAndPublishEvents_FlushErrorReturnsZero(t *testing.T) {
	tracker := &fakeEventTracker{flushErr: errors.New("wasm flush failed")}
	client := &fakeEventsClient{}
	provider := newDrainTestProvider(tracker, client)

	if n := provider.flushAndPublishEvents(context.Background()); n != 0 {
		t.Errorf("Expected 0 events on flush error, got %d", n)
	}
	if client.calls != 0 {
		t.Errorf("Expected no publish calls on flush error, got %d", client.calls)
	}
}
