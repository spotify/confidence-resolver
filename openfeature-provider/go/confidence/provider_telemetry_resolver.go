package confidence

import (
	"context"
	"sync"

	lr "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/local_resolver"
	resolvertypes "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
)

// providerTelemetryResolver owns provider-scoped telemetry above pooling and recovery. The inner
// resolver stack is constructed with writeLogs, so every pooled WASM instance shares one init
// state. Close forces an init-only request when no earlier full flush produced one.
type providerTelemetryResolver struct {
	delegate lr.LocalResolver
	logSink  lr.LogSink
	labels   map[string]string
	sdk      *resolvertypes.Sdk

	mu       sync.Mutex
	initSent bool
}

func newProviderTelemetryResolver(
	logSink lr.LogSink,
	labels map[string]string,
	innerFactory func(lr.LogSink) lr.LocalResolver,
) lr.LocalResolver {
	r := &providerTelemetryResolver{
		logSink: logSink,
		labels:  labels,
		sdk: &resolvertypes.Sdk{
			Sdk:     &resolvertypes.Sdk_Id{Id: resolvertypes.SdkId_SDK_ID_GO_LOCAL_PROVIDER},
			Version: Version,
		},
	}
	r.delegate = innerFactory(r.writeLogs)
	return r
}

func (r *providerTelemetryResolver) writeLogs(logs *resolverv1.WriteFlagLogsRequest) {
	r.mu.Lock()
	defer r.mu.Unlock()

	if !r.initSent {
		r.addInitTelemetry(logs)
	}
	r.logSink(logs)
	r.initSent = true
}

func (r *providerTelemetryResolver) emitInitIfPending() {
	r.mu.Lock()
	defer r.mu.Unlock()

	if r.initSent {
		return
	}
	logs := &resolverv1.WriteFlagLogsRequest{}
	r.addInitTelemetry(logs)
	r.logSink(logs)
	r.initSent = true
}

func (r *providerTelemetryResolver) addInitTelemetry(logs *resolverv1.WriteFlagLogsRequest) {
	if logs.TelemetryData == nil {
		logs.TelemetryData = &resolverv1.TelemetryData{}
	}
	logs.TelemetryData.Sdk = r.sdk
	logs.TelemetryData.ProviderInitRate = append(
		logs.TelemetryData.ProviderInitRate,
		&resolverv1.TelemetryData_ProviderInitRate{Count: 1, Labels: r.labels},
	)
}

func (r *providerTelemetryResolver) SetResolverState(request *wasm.SetResolverStateRequest) error {
	return r.delegate.SetResolverState(request)
}

func (r *providerTelemetryResolver) ResolveProcess(request *wasm.ResolveProcessRequest) (*wasm.ResolveProcessResponse, error) {
	return r.delegate.ResolveProcess(request)
}

func (r *providerTelemetryResolver) RegisterResolve(request *wasm.RegisterResolveRequest) {
	r.delegate.RegisterResolve(request)
}

func (r *providerTelemetryResolver) ApplyFlags(request *resolvertypes.ApplyFlagsRequest) error {
	return r.delegate.ApplyFlags(request)
}

func (r *providerTelemetryResolver) FlushAllLogs() error    { return r.delegate.FlushAllLogs() }
func (r *providerTelemetryResolver) FlushAssignLogs() error { return r.delegate.FlushAssignLogs() }
func (r *providerTelemetryResolver) PrometheusSnapshot(bucketsPerDecade uint32, openmetrics bool) string {
	return r.delegate.PrometheusSnapshot(bucketsPerDecade, openmetrics)
}

func (r *providerTelemetryResolver) Close(ctx context.Context) error {
	err := r.delegate.Close(ctx)
	r.emitInitIfPending()
	return err
}
