package confidence

import (
	"context"
	"log/slog"
	"os"
	"testing"

	"github.com/open-feature/go-sdk/openfeature"
	lr "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/local_resolver"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
	tu "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
)

// TestLocalResolverProvider_DropStateBeforeValidationConfig verifies that the
// DropStateBeforeValidation option (and the default) is forwarded on every
// SetResolverStateRequest: the provider-level surface for the guest's
// drop_state_before_validation flag.
func TestLocalResolverProvider_DropStateBeforeValidationConfig(t *testing.T) {
	for _, tc := range []struct {
		name     string
		option   Option
		expected bool
	}{
		{name: "default keeps previous state until the new one validates", option: nil, expected: false},
		{name: "opt-out drops the previous state before validation", option: WithDropStateBeforeValidation(), expected: true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			mockedResolver := &tu.MockedLocalResolver{
				Response: &wasm.ResolveProcessResponse{
					Result: &wasm.ResolveProcessResponse_Resolved_{
						Resolved: &wasm.ResolveProcessResponse_Resolved{
							Response: tu.CreateTutorialFeatureResponse(),
						},
					},
				},
			}
			stateProvider := &tu.StateProviderMock{
				State:     tu.LoadTestResolverState(t),
				AccountID: tu.LoadTestAccountID(t),
			}
			resolverSupplier := wrapResolverSupplierWithMaterializations(func(ctx context.Context, logSink lr.LogSink) lr.LocalResolver {
				return mockedResolver
			}, newUnsupportedMaterializationStore())

			var opts []Option
			if tc.option != nil {
				opts = append(opts, tc.option)
			}
			provider := newLocalResolverProvider(
				resolverSupplier,
				stateProvider,
				&tu.MockFlagLogger{},
				tu.TestClientSecret,
				slog.New(slog.NewTextHandler(os.Stderr, nil)),
				opts...,
			)
			if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
				t.Fatalf("Init: %v", err)
			}
			if mockedResolver.LastSetResolverState == nil {
				t.Fatal("expected SetResolverState to be called during Init")
			}
			if got := mockedResolver.LastSetResolverState.DropStateBeforeValidation; got != tc.expected {
				t.Fatalf("DropStateBeforeValidation on SetResolverStateRequest = %v, want %v", got, tc.expected)
			}
		})
	}
}
