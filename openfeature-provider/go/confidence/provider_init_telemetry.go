package confidence

import (
	"sync"

	lr "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/local_resolver"
	resolvertypes "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
)

// newProviderInitLogSink owns provider-init telemetry above the resolver pool. All pooled and
// recovered WASM instances share this sink, so one provider emits exactly one init event.
func newProviderInitLogSink(delegate lr.LogSink, labels map[string]string) lr.LogSink {
	var mu sync.Mutex
	sent := false
	sdk := &resolvertypes.Sdk{
		Sdk:     &resolvertypes.Sdk_Id{Id: resolvertypes.SdkId_SDK_ID_GO_LOCAL_PROVIDER},
		Version: Version,
	}

	return func(logs *resolverv1.WriteFlagLogsRequest) {
		mu.Lock()
		defer mu.Unlock()

		if !sent {
			if logs.TelemetryData == nil {
				logs.TelemetryData = &resolverv1.TelemetryData{}
			}
			logs.TelemetryData.Sdk = sdk
			logs.TelemetryData.ProviderInitRate = append(
				logs.TelemetryData.ProviderInitRate,
				&resolverv1.TelemetryData_ProviderInitRate{Count: 1, Labels: labels},
			)
		}

		delegate(logs)
		sent = true
	}
}
