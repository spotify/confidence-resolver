package confidence

import (
	"testing"

	resolvertypes "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
)

func TestProviderInitLogSinkEmitsOnceAcrossResolvers(t *testing.T) {
	var captured []*resolverv1.WriteFlagLogsRequest
	sink := newProviderInitLogSink(
		func(logs *resolverv1.WriteFlagLogsRequest) { captured = append(captured, logs) },
		map[string]string{"encryption": "true"},
	)

	sink(&resolverv1.WriteFlagLogsRequest{})
	sink(&resolverv1.WriteFlagLogsRequest{})
	sink(&resolverv1.WriteFlagLogsRequest{})

	initEvents := 0
	for _, request := range captured {
		initEvents += len(request.GetTelemetryData().GetProviderInitRate())
	}
	if initEvents != 1 {
		t.Fatalf("expected one provider init event, got %d", initEvents)
	}
	telemetry := captured[0].GetTelemetryData()
	if telemetry.GetSdk().GetId() != resolvertypes.SdkId_SDK_ID_GO_LOCAL_PROVIDER {
		t.Fatalf("unexpected SDK ID: %v", telemetry.GetSdk().GetId())
	}
	if telemetry.GetSdk().GetVersion() != Version {
		t.Fatalf("unexpected SDK version: %q", telemetry.GetSdk().GetVersion())
	}
}

func TestProviderInitLogSinkRetriesAfterDelegatePanic(t *testing.T) {
	attempts := 0
	var retried *resolverv1.WriteFlagLogsRequest
	sink := newProviderInitLogSink(func(logs *resolverv1.WriteFlagLogsRequest) {
		attempts++
		if attempts == 1 {
			panic("send failed")
		}
		retried = logs
	}, nil)

	func() {
		defer func() { _ = recover() }()
		sink(&resolverv1.WriteFlagLogsRequest{})
	}()
	sink(&resolverv1.WriteFlagLogsRequest{})

	if got := len(retried.GetTelemetryData().GetProviderInitRate()); got != 1 {
		t.Fatalf("expected init telemetry to be retried, got %d events", got)
	}
}
