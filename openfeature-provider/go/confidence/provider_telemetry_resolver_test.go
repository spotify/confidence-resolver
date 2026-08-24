package confidence

import (
	"context"
	"testing"

	lr "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/local_resolver"
	resolvertypes "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
)

func TestProviderTelemetryResolverEmitsOnceAcrossPooledFlushes(t *testing.T) {
	var captured []*resolverv1.WriteFlagLogsRequest
	resolver := newProviderTelemetryResolver(
		func(logs *resolverv1.WriteFlagLogsRequest) { captured = append(captured, logs) },
		map[string]string{"encryption": "true"},
		func(sink lr.LogSink) lr.LocalResolver { return &telemetryTestResolver{sink: sink, flushCount: 3} },
	)

	if err := resolver.FlushAllLogs(); err != nil {
		t.Fatal(err)
	}

	if got := providerInitEventCount(captured); got != 1 {
		t.Fatalf("expected one provider init event across pooled flushes, got %d", got)
	}
	telemetry := captured[0].GetTelemetryData()
	if telemetry.GetSdk().GetId() != resolvertypes.SdkId_SDK_ID_GO_LOCAL_PROVIDER {
		t.Fatalf("unexpected SDK ID: %v", telemetry.GetSdk().GetId())
	}
	if telemetry.GetSdk().GetVersion() != Version {
		t.Fatalf("unexpected SDK version: %q", telemetry.GetSdk().GetVersion())
	}
}

func TestProviderTelemetryResolverCloseEmitsWithoutResolve(t *testing.T) {
	var captured []*resolverv1.WriteFlagLogsRequest
	resolver := newProviderTelemetryResolver(
		func(logs *resolverv1.WriteFlagLogsRequest) { captured = append(captured, logs) },
		map[string]string{"encryption": "true"},
		func(sink lr.LogSink) lr.LocalResolver { return &telemetryTestResolver{sink: sink} },
	)

	if err := resolver.Close(context.Background()); err != nil {
		t.Fatal(err)
	}

	if got := providerInitEventCount(captured); got != 1 {
		t.Fatalf("expected shutdown to emit one provider init event, got %d", got)
	}
}

func TestProviderTelemetryResolverOwnsSingleInitSample(t *testing.T) {
	resolver := &providerTelemetryResolver{
		labels: map[string]string{"encryption": "true"},
		sdk:    &resolvertypes.Sdk{Version: "test-version"},
	}
	logs := &resolverv1.WriteFlagLogsRequest{
		TelemetryData: &resolverv1.TelemetryData{
			ProviderInitRate: []*resolverv1.TelemetryData_ProviderInitRate{
				{Count: 1, Labels: map[string]string{"existing": "true"}},
			},
		},
	}

	resolver.addInitTelemetry(logs)

	got := logs.GetTelemetryData().GetProviderInitRate()
	if len(got) != 1 {
		t.Fatalf("expected exactly one provider init sample, got %d", len(got))
	}
	if got[0].GetLabels()["encryption"] != "true" {
		t.Fatalf("expected provider-owned labels, got %v", got[0].GetLabels())
	}
}

func TestProviderTelemetryResolverRetriesAfterSinkFailure(t *testing.T) {
	attempts := 0
	var captured []*resolverv1.WriteFlagLogsRequest
	resolver := newProviderTelemetryResolver(
		func(logs *resolverv1.WriteFlagLogsRequest) {
			attempts++
			if attempts == 1 {
				panic("send failed")
			}
			captured = append(captured, logs)
		},
		nil,
		func(sink lr.LogSink) lr.LocalResolver { return &telemetryTestResolver{sink: sink, flushCount: 1} },
	)

	func() {
		defer func() { _ = recover() }()
		_ = resolver.FlushAllLogs()
	}()
	if err := resolver.FlushAllLogs(); err != nil {
		t.Fatal(err)
	}

	if got := providerInitEventCount(captured); got != 1 {
		t.Fatalf("expected init telemetry to be retried, got %d events", got)
	}
}

func providerInitEventCount(requests []*resolverv1.WriteFlagLogsRequest) int {
	count := 0
	for _, request := range requests {
		count += len(request.GetTelemetryData().GetProviderInitRate())
	}
	return count
}

type telemetryTestResolver struct {
	sink       lr.LogSink
	flushCount int
}

func (r *telemetryTestResolver) SetResolverState(*wasm.SetResolverStateRequest) error { return nil }
func (r *telemetryTestResolver) ResolveProcess(*wasm.ResolveProcessRequest) (*wasm.ResolveProcessResponse, error) {
	return &wasm.ResolveProcessResponse{}, nil
}
func (r *telemetryTestResolver) RegisterResolve(*wasm.RegisterResolveRequest)      {}
func (r *telemetryTestResolver) ApplyFlags(*resolvertypes.ApplyFlagsRequest) error { return nil }
func (r *telemetryTestResolver) FlushAllLogs() error {
	for range r.flushCount {
		r.sink(&resolverv1.WriteFlagLogsRequest{})
	}
	return nil
}
func (r *telemetryTestResolver) FlushAssignLogs() error                 { return nil }
func (r *telemetryTestResolver) PrometheusSnapshot(uint32, bool) string { return "" }
func (r *telemetryTestResolver) Close(context.Context) error            { return nil }
