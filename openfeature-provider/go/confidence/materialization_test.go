package confidence

import (
	"context"
	"errors"
	"testing"
	"time"

	lr "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/local_resolver"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
	tu "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
)

func newMaterializationSupportedResolver(store MaterializationStore, innerResolver lr.LocalResolver) *materializationSupportedResolver {
	return &materializationSupportedResolver{
		store:   store,
		current: innerResolver,
	}
}

func TestMaterializationLocalResolverProvider_EmitsErrorFromInnerResolver(t *testing.T) {
	expectedErr := "simulated inner resolver error"
	mockedStore := newUnsupportedMaterializationStore()
	mockedResolver := &tu.MockedLocalResolver{
		Err: errors.New(expectedErr),
	}

	request := tu.CreateResolveProcessRequest(tu.CreateTutorialFeatureRequest())

	resolver := newMaterializationSupportedResolver(mockedStore, mockedResolver)
	_, err := resolver.ResolveProcess(request)
	if err == nil || err.Error() != expectedErr {
		t.Fatalf("expected error %q, got %v", expectedErr, err)
	}
}

func TestMaterializationLocalResolverProvider_WorksWithoutMaterializations(t *testing.T) {
	mockedStore := newUnsupportedMaterializationStore()
	mockedResolver := &tu.MockedLocalResolver{
		Response: &wasm.ResolveProcessResponse{
			Result: &wasm.ResolveProcessResponse_Resolved_{
				Resolved: &wasm.ResolveProcessResponse_Resolved{
					Response: tu.CreateTutorialFeatureResponse(),
				},
			},
		},
	}

	request := tu.CreateResolveProcessRequest(tu.CreateTutorialFeatureRequest())

	resolver := newMaterializationSupportedResolver(mockedStore, mockedResolver)

	response, err := resolver.ResolveProcess(request)
	if err != nil {
		t.Fatalf("expected no error, got %v", err)
	}
	if response == nil {
		t.Fatal("expected non-nil response")
	}
	resolved := response.GetResolved()
	if resolved == nil || resolved.Response == nil {
		t.Fatalf("expected resolved response, got: %#v", response)
	}
	flags := resolved.Response.GetResolvedFlags()
	if len(flags) != 1 {
		t.Fatalf("expected 1 resolved flag, got %d", len(flags))
	}
	flag := flags[0]
	if got, want := flag.GetFlag(), "flags/tutorial-feature"; got != want {
		t.Fatalf("unexpected flag id: got %q want %q", got, want)
	}
	if got, want := flag.GetVariant(), "flags/tutorial-feature/variants/on"; got != want {
		t.Fatalf("unexpected variant: got %q want %q", got, want)
	}
	val := flag.GetValue()
	if val == nil || val.Fields == nil {
		t.Fatalf("expected non-nil value with fields, got: %#v", val)
	}
	enabledField, ok := val.Fields["enabled"]
	if !ok {
		t.Fatalf("expected value to contain 'enabled' field")
	}
	if enabledField.GetBoolValue() != true {
		t.Fatalf("expected enabled=true, got %v", enabledField.GetBoolValue())
	}
}

func TestMaterializationLocalResolverProvider_ReadsStoredMaterializationsCorrectly(t *testing.T) {
	inMemoryStore := newInMemoryMaterializationStore(nil)
	inMemoryStore.Write(context.Background(), []WriteOp{newWriteOpVariant("experiment_v1", "test-user-123", "flags/sticky-test-flag/rules/sticky-rule", "flags/sticky-test-flag/variants/on")})

	mockedResolver := &tu.MockedLocalResolver{
		Responses: []*wasm.ResolveProcessResponse{
			// First call: Suspended, needs materializations
			{
				Result: &wasm.ResolveProcessResponse_Suspended_{
					Suspended: &wasm.ResolveProcessResponse_Suspended{
						MaterializationsToRead: []*wasm.MaterializationRecord{
							{
								Unit:            "test-user-123",
								Materialization: "experiment_v1",
								Rule:            "flags/sticky-test-flag/rules/sticky-rule",
							},
						},
						State: []byte{1, 2, 3}, // opaque continuation
					},
				},
			},
			// Second call (resume): Resolved
			{
				Result: &wasm.ResolveProcessResponse_Resolved_{
					Resolved: &wasm.ResolveProcessResponse_Resolved{
						Response: tu.CreateTutorialFeatureResponse(),
					},
				},
			},
		},
	}

	request := tu.CreateResolveProcessRequest(tu.CreateTutorialFeatureRequest())
	resolver := newMaterializationSupportedResolver(inMemoryStore, mockedResolver)

	response, err := resolver.ResolveProcess(request)
	if err != nil {
		t.Fatalf("expected no error, got %v", err)
	}
	if response == nil {
		t.Fatal("expected non-nil response")
	}

	if len(inMemoryStore.ReadCalls()) != 1 {
		t.Fatalf("expected 1 read call to materialization store, got %d", len(inMemoryStore.ReadCalls()))
	}
	readOps := inMemoryStore.ReadCalls()[0]
	if len(readOps) != 1 {
		t.Fatalf("expected 1 read op in materialization store call, got %d", len(readOps))
	}
	readOp, ok := readOps[0].(*ReadOpVariant)
	if !ok {
		t.Fatalf("expected ReadOpVariant type, got %T", readOps[0])
	}
	if got, want := readOp.Materialization(), "experiment_v1"; got != want {
		t.Fatalf("unexpected materialization id: got %q want %q", got, want)
	}
	if got, want := readOp.Unit(), "test-user-123"; got != want {
		t.Fatalf("unexpected unit id: got %q want %q", got, want)
	}
	if got, want := readOp.Rule(), "flags/sticky-test-flag/rules/sticky-rule"; got != want {
		t.Fatalf("unexpected rule id: got %q want %q", got, want)
	}
}

func TestMaterializationLocalResolverProvider_WritesMaterializationsCorrectly(t *testing.T) {
	inMemoryStore := newInMemoryMaterializationStore(nil)
	mockedResolver := &tu.MockedLocalResolver{
		Response: &wasm.ResolveProcessResponse{
			Result: &wasm.ResolveProcessResponse_Resolved_{
				Resolved: &wasm.ResolveProcessResponse_Resolved{
					Response: tu.CreateTutorialFeatureResponse(),
					MaterializationsToWrite: []*wasm.MaterializationRecord{
						{
							Materialization: "experiment_v1",
							Unit:            "test-user-123",
							Rule:            "flags/sticky-test-flag/rules/sticky-rule",
							Variant:         "flags/sticky-test-flag/variants/on",
						},
					},
				},
			},
		},
	}

	request := tu.CreateResolveProcessRequest(tu.CreateTutorialFeatureRequest())
	resolver := newMaterializationSupportedResolver(inMemoryStore, mockedResolver)

	response, err := resolver.ResolveProcess(request)
	if err != nil {
		t.Fatalf("expected no error, got %v", err)
	}
	if response == nil {
		t.Fatal("expected non-nil response")
	}
	// wait for goroutines to finish writing
	time.Sleep(10 * time.Millisecond)

	if len(inMemoryStore.WriteCalls()) != 1 {
		t.Fatalf("expected 1 write call to materialization store, got %d", len(inMemoryStore.WriteCalls()))
	}
	writeOps := inMemoryStore.WriteCalls()[0]
	if len(writeOps) != 1 {
		t.Fatalf("expected 1 write op in materialization store call, got %d", len(writeOps))
	}
	writeOp, ok := writeOps[0].(*WriteOpVariant)
	if !ok {
		t.Fatalf("expected WriteOpVariant type, got %T", writeOps[0])
	}
	if got, want := writeOp.Materialization(), "experiment_v1"; got != want {
		t.Fatalf("unexpected materialization id: got %q want %q", got, want)
	}
	if got, want := writeOp.Unit(), "test-user-123"; got != want {
		t.Fatalf("unexpected unit id: got %q want %q", got, want)
	}
	if got, want := writeOp.Rule(), "flags/sticky-test-flag/rules/sticky-rule"; got != want {
		t.Fatalf("unexpected rule id: got %q want %q", got, want)
	}
	if got, want := writeOp.Variant(), "flags/sticky-test-flag/variants/on"; got != want {
		t.Fatalf("unexpected variant: got %q want %q", got, want)
	}
}

func TestMaterializationLocalResolverProvider_RejectsSecondSuspend(t *testing.T) {
	inMemoryStore := newInMemoryMaterializationStore(nil)
	defer inMemoryStore.Close()
	inMemoryStore.Write(context.Background(), []WriteOp{newWriteOpVariant("experiment_v1", "test-user-123", "flags/sticky-test-flag/rules/sticky-rule", "flags/sticky-test-flag/variants/on")})

	// Always return Suspended — should fail on second suspend after resume
	mockedResolver := &tu.MockedLocalResolver{
		Response: &wasm.ResolveProcessResponse{
			Result: &wasm.ResolveProcessResponse_Suspended_{
				Suspended: &wasm.ResolveProcessResponse_Suspended{
					MaterializationsToRead: []*wasm.MaterializationRecord{
						{
							Unit:            "test-user-123",
							Materialization: "experiment_v1",
							Rule:            "flags/sticky-test-flag/rules/sticky-rule",
						},
					},
					State: []byte{1, 2, 3},
				},
			},
		},
	}

	request := tu.CreateResolveProcessRequest(tu.CreateTutorialFeatureRequest())
	resolver := newMaterializationSupportedResolver(inMemoryStore, mockedResolver)

	response, err := resolver.ResolveProcess(request)
	if response != nil {
		t.Fatal("expected nil response")
	}
	expectedErr := "unexpected second suspend after resume"
	if err == nil || err.Error() != expectedErr {
		t.Fatalf("expected error %q, got %v", expectedErr, err)
	}
}
