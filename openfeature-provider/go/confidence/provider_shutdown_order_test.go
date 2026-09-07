package confidence

import (
	"context"
	"io"
	"log/slog"
	"testing"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/events"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/eventswasm"
	resolverproto "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
)

// orderRecordingResolver records when its final-flush Close runs, so a test can
// assert it happens after the event drain.
type orderRecordingResolver struct {
	order *[]string
}

func (r *orderRecordingResolver) SetResolverState(*wasm.SetResolverStateRequest) error { return nil }

func (r *orderRecordingResolver) ResolveProcess(*wasm.ResolveProcessRequest) (*wasm.ResolveProcessResponse, error) {
	return &wasm.ResolveProcessResponse{}, nil
}

func (r *orderRecordingResolver) RegisterResolve(*wasm.RegisterResolveRequest) {}

func (r *orderRecordingResolver) ApplyFlags(*resolverproto.ApplyFlagsRequest) error { return nil }

func (r *orderRecordingResolver) FlushAllLogs() error { return nil }

func (r *orderRecordingResolver) FlushAssignLogs() error { return nil }

func (r *orderRecordingResolver) PrometheusSnapshot(uint32, bool) string { return "" }

func (r *orderRecordingResolver) Close(context.Context) error {
	*r.order = append(*r.order, "resolver.Close")
	return nil
}

// orderRecordingTracker records when the event drain flushes.
type orderRecordingTracker struct {
	order            *[]string
	remainingBatches int
}

func (t *orderRecordingTracker) TrackEvent(*eventswasm.TrackEventRequest) error { return nil }

func (t *orderRecordingTracker) FlushEvents() (*eventswasm.FlushEventsResponse, error) {
	*t.order = append(*t.order, "drainEvents")
	if t.remainingBatches <= 0 {
		return &eventswasm.FlushEventsResponse{}, nil
	}
	t.remainingBatches--
	return &eventswasm.FlushEventsResponse{Events: []*events.Event{{EventDefinition: "eventDefinitions/test"}}}, nil
}

func (t *orderRecordingTracker) Close() error { return nil }

// Event delivery outcomes are reported on the next WriteFlagLogs, so the event
// drain must run before the resolver's final flush — otherwise the last batch's
// published/rejected/succeeded/failed counters are stranded in process-local
// atomics and never reach the backend.
func TestShutdown_DrainsEventsBeforeResolverClose(t *testing.T) {
	var order []string
	provider := &LocalResolverProvider{
		clientSecret: "test-secret",
		logger:       slog.New(slog.NewTextHandler(io.Discard, &slog.HandlerOptions{Level: slog.LevelError})),
		eventTracker: &orderRecordingTracker{order: &order, remainingBatches: 1},
		eventsClient: &fakeEventsClient{},
		resolver:     &orderRecordingResolver{order: &order},
	}

	provider.Shutdown()

	drainAt, closeAt := -1, -1
	for i, step := range order {
		if step == "drainEvents" && drainAt == -1 {
			drainAt = i
		}
		if step == "resolver.Close" {
			closeAt = i
		}
	}
	if drainAt == -1 {
		t.Fatalf("event drain never ran during Shutdown; order=%v", order)
	}
	if closeAt == -1 {
		t.Fatalf("resolver.Close never ran during Shutdown; order=%v", order)
	}
	if drainAt > closeAt {
		t.Errorf("events drained after the resolver's final flush, so the last batch's telemetry counters are lost; order=%v", order)
	}
}
