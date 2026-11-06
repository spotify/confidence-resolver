package local_resolver

import (
	"context"
	"testing"

	"google.golang.org/protobuf/encoding/protowire"
	"google.golang.org/protobuf/proto"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/admin"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
	tu "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
)

// corruptStateForValidationFailure returns a resolver state that decodes as
// protobuf but fails the guest's ResolverState::from_proto validation: a
// packed bitset with full_bitset=false hits from_proto's failure arm.
func corruptStateForValidationFailure(t *testing.T) []byte {
	t.Helper()
	raw := tu.LoadTestResolverState(t)
	var state admin.ResolverState
	if err := proto.Unmarshal(raw, &state); err != nil {
		t.Fatalf("failed to unmarshal fixture state: %v", err)
	}
	// from_proto processes every packed bitset regardless of whether a rule
	// references it, and full_bitset=false is rejected by its failure arm.
	state.Bitsets = append(state.Bitsets, &admin.ResolverState_PackedBitset{
		Segment: "corrupt/for-test",
		Bitset:  &admin.ResolverState_PackedBitset_FullBitset{FullBitset: false},
	})
	corrupt, err := proto.Marshal(&state)
	if err != nil {
		t.Fatalf("failed to marshal corrupt state: %v", err)
	}
	return corrupt
}

// tryResolveTutorialFeature resolves the tutorial feature and reports which
// variant came back. resolved=false means the response carried no resolved
// flag (error response or empty resolution).
func tryResolveTutorialFeature(t *testing.T, r LocalResolver) (variant string, resolved bool) {
	t.Helper()
	request := tu.CreateResolveProcessRequest(tu.CreateTutorialFeatureRequest())
	processResponse, err := r.ResolveProcess(request)
	if err != nil {
		return "", false
	}
	response := processResponse.GetResolved().GetResponse()
	if response == nil || len(response.ResolvedFlags) == 0 {
		return "", false
	}
	return response.ResolvedFlags[0].Variant, true
}

// TestFailedUpdateKeepsPreviousStateByDefault verifies the new guest default:
// a state update that fails validation keeps the previously-installed state
// serving — no stateless window.
func TestFailedUpdateKeepsPreviousStateByDefault(t *testing.T) {
	ctx := context.Background()
	good := tu.LoadTestResolverState(t)
	corrupt := corruptStateForValidationFailure(t)
	accountId := tu.LoadTestAccountID(t)

	factory := DefaultResolverFactory(NoOpLogSink, LocalResolverConfig{})
	resolver := factory.New()
	defer resolver.Close(ctx)

	if err := resolver.SetResolverState(&wasm.SetResolverStateRequest{State: good, AccountId: accountId}); err != nil {
		t.Fatalf("failed to install initial state: %v", err)
	}
	variant, resolved := tryResolveTutorialFeature(t, resolver)
	if !resolved || variant != "flags/tutorial-feature/variants/exciting-welcome" {
		t.Fatalf("initial resolve: resolved=%v variant=%q", resolved, variant)
	}

	updateErr := resolver.SetResolverState(&wasm.SetResolverStateRequest{State: corrupt, AccountId: accountId})
	if updateErr == nil {
		t.Fatal("expected the corrupt state update to fail validation")
	}

	variant, resolved = tryResolveTutorialFeature(t, resolver)
	if !resolved || variant != "flags/tutorial-feature/variants/exciting-welcome" {
		t.Fatalf("default (no drop) should keep serving the previous state after a failed update; got resolved=%v variant=%q", resolved, variant)
	}
	t.Logf("✓ failed update kept the previous state by default (update error: %v)", updateErr)
}

// TestDropStateBeforeValidationOptOut verifies the opt-out: with
// drop_state_before_validation=true the guest frees the previous state first,
// reproducing the memory-optimized behavior where a failed update leaves the
// resolver stateless until the next successful update.
func TestDropStateBeforeValidationOptOut(t *testing.T) {
	ctx := context.Background()
	good := tu.LoadTestResolverState(t)
	corrupt := corruptStateForValidationFailure(t)
	accountId := tu.LoadTestAccountID(t)

	factory := DefaultResolverFactory(NoOpLogSink, LocalResolverConfig{})
	resolver := factory.New()
	defer resolver.Close(ctx)

	if err := resolver.SetResolverState(&wasm.SetResolverStateRequest{State: good, AccountId: accountId}); err != nil {
		t.Fatalf("failed to install initial state: %v", err)
	}
	variant, resolved := tryResolveTutorialFeature(t, resolver)
	if !resolved || variant != "flags/tutorial-feature/variants/exciting-welcome" {
		t.Fatalf("initial resolve: resolved=%v variant=%q", resolved, variant)
	}

	if err := resolver.SetResolverState(&wasm.SetResolverStateRequest{
		State:                     corrupt,
		AccountId:                 accountId,
		DropStateBeforeValidation: true,
	}); err == nil {
		t.Fatal("expected the corrupt state update to fail validation")
	}

	variant, resolved = tryResolveTutorialFeature(t, resolver)
	if resolved {
		t.Fatalf("expected the stateless window with the opt-out; got variant=%q", variant)
	}
	t.Logf("✓ drop_state_before_validation opt-out reproduced the stateless window (default remains no-drop)")
}

// paddedState returns the fixture state padded with a large unknown field so
// page growth is visible (same technique as TestWasmMemoryBoundedAcrossStateUpdates).
func paddedState(t *testing.T) []byte {
	t.Helper()
	const padding = 32 << 20
	state := tu.LoadTestResolverState(t)
	state = append(state, 0xc2, 0xd3, 0x07) // field 15000, wire type 2
	state = protowire.AppendVarint(state, padding)
	state = append(state, make([]byte, padding)...)
	return state
}

// TestWasmMemoryStateUpdateModes measures WASM memory across repeated state
// updates in both modes and reports the arrays, so the memory cost of the
// default (keep previous state until the new one validates) versus the
// drop_state_before_validation opt-out is visible.
func TestWasmMemoryStateUpdateModes(t *testing.T) {
	for _, mode := range []struct {
		name  string
		drop  bool
		limit func(stateLen int) uint32
	}{
		{
			name: "default-keep-previous",
			drop: false,
			// Old and new state coexist during the swap: bounded by both
			// states' encodings plus the decoded structures and slack.
			limit: func(stateLen int) uint32 { return uint32(4*stateLen + 16<<20) },
		},
		{
			name: "drop-state-before-validation",
			drop: true,
			// The PR's original budget: one decoded state plus the old one's
			// structures.
			limit: func(stateLen int) uint32 { return uint32(2*stateLen + 16<<20) },
		},
	} {
		t.Run(mode.name, func(t *testing.T) {
			factory := NewWasmResolverFactory(NoOpLogSink, false)
			defer factory.Close(context.Background())
			resolver := factory.New()
			defer resolver.Close(context.Background())
			wasmResolver := resolver.(*WasmResolver)

			state := paddedState(t)
			request := &wasm.SetResolverStateRequest{
				State:                     state,
				AccountId:                 tu.LoadTestAccountID(t),
				DropStateBeforeValidation: mode.drop,
			}
			resolve := tu.CreateResolveProcessRequest(tu.CreateTutorialFeatureRequest())

			var sizes []uint32
			for i := 0; i < 5; i++ {
				if err := wasmResolver.SetResolverState(request); err != nil {
					t.Fatalf("SetResolverState %d: %v", i+1, err)
				}
				for j := 0; j < 100; j++ {
					if _, err := wasmResolver.ResolveProcess(resolve); err != nil {
						t.Fatalf("ResolveProcess: %v", err)
					}
				}
				sizes = append(sizes, wasmResolver.instance.Memory().Size())
			}
			t.Logf("WASM memory after each load: %v bytes", sizes)

			if last := sizes[len(sizes)-1]; last > sizes[1] {
				t.Errorf("memory grew from %d to %d bytes between the second and fifth load", sizes[1], last)
			}
			if limit := mode.limit(len(state)); sizes[len(sizes)-1] > limit {
				t.Errorf("WASM memory %d bytes exceeds mode budget %d", sizes[len(sizes)-1], limit)
			}
		})
	}
}
