package confidence

import (
	"context"
	"io"
	"log/slog"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
	lr "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/local_resolver"
	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
	tu "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
)

func newApplyTimeWasmProvider(t testing.TB, sink lr.LogSink) *LocalResolverProvider {
	t.Helper()
	state, err := os.ReadFile("../../../data/resolver_state_current.pb")
	if err != nil {
		t.Fatal(err)
	}
	account, err := os.ReadFile("../../../data/account_id")
	if err != nil {
		t.Fatal(err)
	}
	r := lr.NewLocalResolverWithPoolSize(context.Background(), sink, 1)
	t.Cleanup(func() { _ = r.Close(context.Background()) })
	if err := r.SetResolverState(&wasm.SetResolverStateRequest{
		State: state, AccountId: strings.TrimSpace(string(account)), EnableApplyDedup: true,
	}); err != nil {
		t.Fatal(err)
	}
	p := newLocalResolverProvider(nil, nil, nil, tu.TestClientSecret, slog.New(slog.NewTextHandler(io.Discard, nil)))
	p.resolver = r
	p.ready.Store(true)
	return p
}

func TestWithApplyTimeWasmRecordsOneBackdatedExposure(t *testing.T) {
	var applied []*resolverv1.FlagAssigned_AppliedFlag
	p := newApplyTimeWasmProvider(t, func(logs *resolverv1.WriteFlagLogsRequest) {
		for _, assignment := range logs.FlagAssigned {
			applied = append(applied, assignment.Flags...)
		}
	})
	applyTime := time.Now().Add(-10 * time.Minute)
	ctx := WithApplyTime(t.Context(), applyTime)
	result := p.ObjectEvaluation(ctx, "tutorial-feature", map[string]any{}, openfeature.FlattenedContext{"visitor_id": "tutorial_visitor"})
	if result.Error() != nil || result.Variant != "flags/tutorial-feature/variants/exciting-welcome" {
		t.Fatalf("unexpected evaluation: %+v", result)
	}
	if err := p.resolver.FlushAllLogs(); err != nil {
		t.Fatal(err)
	}
	if len(applied) != 1 || applied[0].Flag != "flags/tutorial-feature" {
		t.Fatalf("expected one tutorial-feature exposure, got %v", applied)
	}
	if delta := applied[0].ApplyTime.AsTime().Sub(applyTime); delta < -time.Second || delta > time.Second {
		t.Fatalf("apply time was not backdated: got %v, want %v", applied[0].ApplyTime, applyTime)
	}
}
