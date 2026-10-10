package confidence

import (
	"context"
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
	lr "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/local_resolver"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	"google.golang.org/protobuf/types/known/timestamppb"
)

func doubleResolveWithApplyTime(p *LocalResolverProvider, ctx context.Context, applyTime time.Time) openfeature.InterfaceResolutionDetail {
	result := p.ObjectEvaluation(ctx, "tutorial-feature", map[string]any{}, openfeature.FlattenedContext{
		"visitor_id": "tutorial_visitor", "_confidence_skip_apply": true,
	})
	if result.Error() != nil {
		return result
	}
	response, err := p.Resolve(ctx, openfeature.FlattenedContext{"visitor_id": "tutorial_visitor"}, []string{"tutorial-feature"}, false)
	if err != nil {
		panic(err)
	}
	for _, flag := range response.ResolvedFlags {
		if flag.ShouldApply {
			if err := p.ApplyFlags(&resolver.ApplyFlagsRequest{
				Flags:        []*resolver.AppliedFlag{{Flag: flag.Flag, ApplyTime: timestamppb.New(applyTime)}},
				ClientSecret: p.clientSecret, ResolveToken: response.ResolveToken, SendTime: timestamppb.Now(),
			}); err != nil {
				panic(err)
			}
		}
	}
	return result
}

func BenchmarkWithApplyTime(b *testing.B) {
	for _, name := range []string{"double_resolve", "single_resolve"} {
		b.Run(name, func(b *testing.B) {
			p := newApplyTimeWasmProvider(b, lr.NoOpLogSink)
			applyTime := time.Now().Add(-10 * time.Minute)
			ctx := WithApplyTime(context.Background(), applyTime)
			b.ReportAllocs()
			for b.Loop() {
				var result openfeature.InterfaceResolutionDetail
				if name == "double_resolve" {
					result = doubleResolveWithApplyTime(p, context.Background(), applyTime)
				} else {
					result = p.ObjectEvaluation(ctx, "tutorial-feature", map[string]any{}, openfeature.FlattenedContext{"visitor_id": "tutorial_visitor"})
				}
				if result.Error() != nil {
					b.Fatal(result.Error())
				}
			}
		})
	}
}
