package confidence

import (
	"context"
	"time"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	"google.golang.org/protobuf/types/known/timestamppb"
)

type applyTimeContextKey struct{}

// WithApplyTime records successful typed evaluations at applyTime instead of resolve time.
// A zero time restores normal application; exposure suppression still takes precedence.
func WithApplyTime(ctx context.Context, applyTime time.Time) context.Context {
	return context.WithValue(ctx, applyTimeContextKey{}, applyTime)
}

func (p *LocalResolverProvider) applyResolvedAtTime(ctx context.Context, token []byte, flag *resolver.ResolvedFlag, applyTime time.Time) {
	if !flag.ShouldApply {
		return
	}
	if err := p.ApplyFlags(&resolver.ApplyFlagsRequest{
		Flags:        []*resolver.AppliedFlag{{Flag: flag.Flag, ApplyTime: timestamppb.New(applyTime)}},
		ClientSecret: p.clientSecret, ResolveToken: token, SendTime: timestamppb.Now(),
	}); err != nil {
		p.logger.WarnContext(ctx, "Failed to apply flag at requested time", "flag", flag.Flag, "error", err)
	}
}
