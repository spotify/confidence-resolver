package local_resolver

import (
	"testing"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
	tu "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
	"google.golang.org/protobuf/types/known/structpb"
)

func TestSetEncryptedResolverState_ResolvesFlags(t *testing.T) {
	encryptedState := tu.LoadTestEncryptedState(t)
	encryptionKey := tu.LoadTestEncryptionKey(t)

	r := NewWasmResolverFactory(NoOpLogSink).New()
	defer r.Close(nil)

	err := r.SetEncryptedResolverState(&wasm.SetEncryptedResolverStateRequest{
		EncryptedState: encryptedState,
		EncryptionKey:  encryptionKey,
		Sdk: &resolver.Sdk{
			Sdk:     &resolver.Sdk_Id{Id: resolver.SdkId_SDK_ID_GO_LOCAL_PROVIDER},
			Version: "0.0.0-test",
		},
	})
	if err != nil {
		t.Fatalf("SetEncryptedResolverState failed: %v", err)
	}

	ctx, _ := structpb.NewStruct(map[string]interface{}{
		"targeting_key": "tutorial_visitor",
		"visitor_id":    "tutorial_visitor",
	})
	resp, err := r.ResolveProcess(&wasm.ResolveProcessRequest{
		Resolve: &wasm.ResolveProcessRequest_DeferredMaterializations{
			DeferredMaterializations: &resolver.ResolveFlagsRequest{
				Flags:             []string{"flags/tutorial-feature"},
				ClientSecret:      "mkjJruAATQWjeY7foFIWfVAcBWnci2YF",
				Apply:             true,
				EvaluationContext: ctx,
			},
		},
	})
	if err != nil {
		t.Fatalf("ResolveProcess failed: %v", err)
	}

	resolved := resp.GetResolved()
	if resolved == nil {
		t.Fatal("Expected resolved response")
	}
	flags := resolved.GetResponse().GetResolvedFlags()
	if len(flags) != 1 {
		t.Fatalf("Expected 1 resolved flag, got %d", len(flags))
	}
	if flags[0].Reason != resolver.ResolveReason_RESOLVE_REASON_MATCH {
		t.Errorf("Expected MATCH, got %v", flags[0].Reason)
	}
}

func TestSetEncryptedResolverState_RejectsWrongKey(t *testing.T) {
	encryptedState := tu.LoadTestEncryptedState(t)

	r := NewWasmResolverFactory(NoOpLogSink).New()
	defer r.Close(nil)

	err := r.SetEncryptedResolverState(&wasm.SetEncryptedResolverStateRequest{
		EncryptedState: encryptedState,
		EncryptionKey:  make([]byte, 32),
	})
	if err == nil {
		t.Fatal("Expected error with wrong key")
	}
}
