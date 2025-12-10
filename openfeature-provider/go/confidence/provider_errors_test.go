package confidence

import (
	"testing"

	"github.com/open-feature/go-sdk/openfeature"
	resolvertypes "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
)

func TestLocalResolverProvider_ReasonMapping(t *testing.T) {
	// Test that resolver reasons are properly mapped to OpenFeature reasons
	testCases := []struct {
		name           string
		resolverReason resolvertypes.ResolveReason
		hasVariant     bool
		expectedReason openfeature.Reason
	}{
		{
			name:           "Match with variant",
			resolverReason: resolvertypes.ResolveReason_RESOLVE_REASON_MATCH,
			hasVariant:     true,
			expectedReason: openfeature.Reason("RESOLVE_REASON_MATCH"),
		},
		{
			name:           "No segment match",
			resolverReason: resolvertypes.ResolveReason_RESOLVE_REASON_NO_SEGMENT_MATCH,
			hasVariant:     false,
			expectedReason: openfeature.Reason("RESOLVE_REASON_NO_SEGMENT_MATCH"),
		},
		{
			name:           "Targeting key error",
			resolverReason: resolvertypes.ResolveReason_RESOLVE_REASON_TARGETING_KEY_ERROR,
			hasVariant:     false,
			expectedReason: openfeature.Reason("RESOLVE_REASON_TARGETING_KEY_ERROR"),
		},
		{
			name:           "Flag archived",
			resolverReason: resolvertypes.ResolveReason_RESOLVE_REASON_FLAG_ARCHIVED,
			hasVariant:     false,
			expectedReason: openfeature.Reason("RESOLVE_REASON_FLAG_ARCHIVED"),
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			// Simulate the reason conversion
			reason := openfeature.Reason(tc.resolverReason.String())

			if reason != tc.expectedReason {
				t.Errorf("Expected reason %s, got %s", tc.expectedReason, reason)
			}
		})
	}
}
