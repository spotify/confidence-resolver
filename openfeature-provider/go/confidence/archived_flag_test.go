package confidence

import (
	"strings"
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
	"github.com/prometheus/common/expfmt"
	"github.com/prometheus/common/model"
	fl "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/flag_logger"
	admin "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/admin"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
	tu "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
	"google.golang.org/protobuf/proto"
)

func TestOpenFeatureArchivedFlag(t *testing.T) {
	t.Parallel()
	var state admin.ResolverState
	if err := proto.Unmarshal(tu.CreateMinimalResolverState(), &state); err != nil {
		t.Fatal(err)
	}
	state.Flags = []*admin.Flag{
		{Name: "flags/retired", State: admin.Flag_ACTIVE, Clients: []string{"clients/test-client"}},
		{Name: "flags/other-client", State: admin.Flag_ARCHIVED, Clients: []string{"clients/other-client"}},
	}
	encode := func() []byte {
		t.Helper()
		data, err := proto.Marshal(&state)
		if err != nil {
			t.Fatal(err)
		}
		return data
	}
	logger := fl.NewCapturingFlagLogger()
	provider, err := newProviderForTest(t.Context(), ProviderTestConfig{
		ClientSecret:      "test-secret",
		StateProvider:     &tu.StateProviderMock{State: encode(), AccountID: "test-account"},
		FlagLogger:        logger,
		ResolverPoolSize:  1,
		StatePollInterval: time.Hour,
	})
	if err != nil {
		t.Fatal(err)
	}
	if err := openfeature.SetNamedProviderAndWait(t.Name(), provider); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := openfeature.SetNamedProviderAndWait(t.Name(), openfeature.NoopProvider{}); err != nil {
			t.Error(err)
		}
	})
	client := openfeature.NewClient(t.Name())
	ctx := t.Context()
	evalCtx := openfeature.NewEvaluationContext("user", nil)
	type value struct {
		Enabled bool `json:"enabled"`
	}
	fallback := value{Enabled: true}
	active, err := client.ObjectValueDetails(ctx, "retired", fallback, evalCtx)
	if err != nil || active.Reason != openfeature.DefaultReason || active.Value != fallback {
		t.Fatalf("active flag without rules: %+v, %v", active, err)
	}
	state.Flags[0].State = admin.Flag_ARCHIVED
	if err := provider.resolver.SetResolverState(&wasm.SetResolverStateRequest{
		State: encode(), AccountId: "test-account",
	}); err != nil {
		t.Fatal(err)
	}
	archived, err := client.ObjectValueDetails(ctx, "retired", fallback, evalCtx)
	if err != nil || archived.ErrorCode != "" || archived.Reason != openfeature.DisabledReason || archived.Value != fallback || archived.Variant != "" {
		t.Errorf("archived object must return fallback with DISABLED and no error: %+v, %v", archived, err)
	}
	property, err := client.BooleanValueDetails(ctx, "retired.enabled", true, evalCtx)
	if err != nil || property.ErrorCode != "" || property.Reason != openfeature.DisabledReason || !property.Value {
		t.Errorf("archived property must return fallback with DISABLED and no error: %+v, %v", property, err)
	}
	for _, key := range []string{"never-created", "other-client"} {
		missing, err := client.ObjectValueDetails(ctx, key, fallback, evalCtx)
		if err == nil || missing.ErrorCode != openfeature.FlagNotFoundCode || missing.Value != fallback {
			t.Errorf("%s must remain FLAG_NOT_FOUND with fallback: %+v, %v", key, missing, err)
		}
	}
	all, err := provider.Resolve(ctx, openfeature.FlattenedContext{}, nil, true)
	if err != nil {
		t.Fatal(err)
	}
	if len(all.GetResolvedFlags()) != 0 {
		t.Errorf("resolve-all must omit archived flags: %v", all.GetResolvedFlags())
	}
	parser := expfmt.NewTextParser(model.UTF8Validation)
	families, err := parser.TextToMetricFamilies(strings.NewReader(provider.GetPrometheusMetrics(SnapshotConfig{})))
	if err != nil {
		t.Fatal(err)
	}
	counts := map[string]float64{}
	for _, metric := range families["confidence_resolves_total"].GetMetric() {
		for _, label := range metric.GetLabel() {
			if label.GetName() == "reason" {
				counts[label.GetValue()] += metric.GetCounter().GetValue()
			}
		}
	}
	if counts["RESOLVE_REASON_FLAG_ARCHIVED"] != 2 || counts["RESOLVE_REASON_FLAG_NOT_FOUND"] != 2 {
		t.Errorf("archived resolutions must stay separate from errors in metrics: %v", counts)
	}
	if err := provider.resolver.FlushAllLogs(); err != nil {
		t.Fatal(err)
	}
	for _, request := range logger.GetCapturedRequests() {
		for _, assignment := range request.GetFlagAssigned() {
			if len(assignment.GetFlags()) != 0 {
				t.Errorf("archived flags must not generate exposures: %v", assignment.GetFlags())
			}
		}
	}
}
