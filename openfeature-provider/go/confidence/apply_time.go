package confidence

import (
	"context"
	"time"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	"google.golang.org/protobuf/types/known/timestamppb"
)

type applyTimeKey struct{}

// WithApplyTime returns a context whose flag evaluations record their exposure
// at t instead of the resolve time. Use it when the treatment applies to
// something timestamped before the resolve, such as a message whose handling
// triggered the evaluation. `_confidence_skip_apply` and
// DisableExposureCollection take precedence.
func WithApplyTime(ctx context.Context, t time.Time) context.Context {
	return context.WithValue(ctx, applyTimeKey{}, t)
}

func applyTimeFromContext(ctx context.Context) (time.Time, bool) {
	t, ok := ctx.Value(applyTimeKey{}).(time.Time)
	return t, ok
}

// applyWithTime records exposure events for the resolved flags, stamped with
// the given apply time instead of the resolve time. The resolve must have been
// made with apply=false so the response carries a resolve token.
func (p *LocalResolverProvider) applyWithTime(flagName string, response *resolver.ResolveFlagsResponse, applyTime time.Time) {
	ts := timestamppb.New(applyTime)
	flags := make([]*resolver.AppliedFlag, 0, len(response.GetResolvedFlags()))
	for _, rf := range response.GetResolvedFlags() {
		if !rf.GetShouldApply() {
			continue
		}
		flags = append(flags, &resolver.AppliedFlag{Flag: rf.GetFlag(), ApplyTime: ts})
	}
	if len(flags) == 0 {
		return
	}
	request := &resolver.ApplyFlagsRequest{
		Flags:        flags,
		ClientSecret: p.clientSecret,
		ResolveToken: response.GetResolveToken(),
		SendTime:     timestamppb.Now(),
		Sdk: &resolver.Sdk{
			Sdk:     &resolver.Sdk_Id{Id: resolver.SdkId_SDK_ID_GO_LOCAL_PROVIDER},
			Version: Version,
		},
	}
	if err := p.resolver.ApplyFlags(request); err != nil {
		p.logger.Error("Failed to apply flag with backdated apply time", "flag", flagName, "error", err)
	}
}
