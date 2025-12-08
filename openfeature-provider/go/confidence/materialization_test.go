package confidence

import (
	"context"
	"errors"
	"testing"
	"time"

	lr "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/local_resolver"
	tu "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/resolver"
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

	request := &resolver.ResolveWithStickyRequest{
		ResolveRequest:          tu.CreateTutorialFeatureRequest(),
		MaterializationsPerUnit: make(map[string]*resolver.MaterializationMap),
		FailFastOnSticky:        false,
		NotProcessSticky:        false,
	}

	resolver := newMaterializationSupportedResolver(mockedStore, mockedResolver)
	_, err := resolver.ResolveWithSticky(request)
	if err == nil || err.Error() != expectedErr {
		t.Fatalf("expected error %q, got %v", expectedErr, err)
	}
}

func TestMaterializationLocalResolverProvider_WorksWithoutMaterializations(t *testing.T) {
	mockedStore := newUnsupportedMaterializationStore()
	mockedResolver := &tu.MockedLocalResolver{
		Response: &resolver.ResolveWithStickyResponse{
			ResolveResult: &resolver.ResolveWithStickyResponse_Success_{
				Success: &resolver.ResolveWithStickyResponse_Success{
					Response: tu.CreateTutorialFeatureResponse(),
				},
			},
		},
	}

	request := &resolver.ResolveWithStickyRequest{
		ResolveRequest:          tu.CreateTutorialFeatureRequest(),
		MaterializationsPerUnit: make(map[string]*resolver.MaterializationMap),
		FailFastOnSticky:        false,
		NotProcessSticky:        false,
	}

	resolver := newMaterializationSupportedResolver(mockedStore, mockedResolver)

	response, err := resolver.ResolveWithSticky(request)
	if err != nil {
		t.Fatalf("expected no error, got %v", err)
	}
	if response == nil {
		t.Fatal("expected non-nil response")
	}
	// Assert resolved variant and value match expectations
	success := response.GetSuccess()
	if success == nil || success.Response == nil {
		t.Fatalf("expected success response, got: %#v", response)
	}
	resolved := success.Response.GetResolvedFlags()
	if len(resolved) != 1 {
		t.Fatalf("expected 1 resolved flag, got %d", len(resolved))
	}
	flag := resolved[0]
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

	// Use empty materialization store that returns no variants
	inMemoryStore := newInMemoryMaterializationStore(nil)
	// Pre-populate store with variant assignment for the test user
	inMemoryStore.Write(context.Background(), []WriteOp{newWriteOpVariant("experiment_v1", "test-user-123", "flags/sticky-test-flag/rules/sticky-rule", "flags/sticky-test-flag/variants/on")})
	mockedResolver := &tu.MockedLocalResolver{
		Responses: []*resolver.ResolveWithStickyResponse{
			{
				ResolveResult: &resolver.ResolveWithStickyResponse_MissingMaterializations_{
					MissingMaterializations: &resolver.ResolveWithStickyResponse_MissingMaterializations{
						Items: []*resolver.ResolveWithStickyResponse_MissingMaterializationItem{
							{
								ReadMaterialization: "experiment_v1",
								Unit:                "test-user-123",
								Rule:                "flags/sticky-test-flag/rules/sticky-rule",
							},
						},
					},
				},
			},
			{
				ResolveResult: &resolver.ResolveWithStickyResponse_Success_{
					Success: &resolver.ResolveWithStickyResponse_Success{
						Response: tu.CreateTutorialFeatureResponse(),
					},
				},
			},
		},
	}

	request := &resolver.ResolveWithStickyRequest{
		ResolveRequest:          tu.CreateTutorialFeatureRequest(),
		MaterializationsPerUnit: make(map[string]*resolver.MaterializationMap),
		FailFastOnSticky:        false,
		NotProcessSticky:        false,
	}
	resolver := newMaterializationSupportedResolver(inMemoryStore, mockedResolver)

	response, err := resolver.ResolveWithSticky(request)
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
	// Use empty materialization store that returns no variants
	inMemoryStore := newInMemoryMaterializationStore(nil)
	mockedResolver := &tu.MockedLocalResolver{
		Response: &resolver.ResolveWithStickyResponse{
			ResolveResult: &resolver.ResolveWithStickyResponse_Success_{
				Success: &resolver.ResolveWithStickyResponse_Success{
					Response: tu.CreateTutorialFeatureResponse(),
					Updates: []*resolver.ResolveWithStickyResponse_MaterializationUpdate{
						{
							WriteMaterialization: "experiment_v1",
							Unit:                 "test-user-123",
							Rule:                 "flags/sticky-test-flag/rules/sticky-rule",
							Variant:              "flags/sticky-test-flag/variants/on",
						},
					},
				},
			},
		},
	}

	request := &resolver.ResolveWithStickyRequest{
		ResolveRequest:          tu.CreateTutorialFeatureRequest(),
		MaterializationsPerUnit: make(map[string]*resolver.MaterializationMap),
		FailFastOnSticky:        false,
		NotProcessSticky:        false,
	}
	resolver := newMaterializationSupportedResolver(inMemoryStore, mockedResolver)

	response, err := resolver.ResolveWithSticky(request)
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

func TestMaterializationLocalResolverProvider_DoesNotRetryBeyondMaxDepth(t *testing.T) {

	// Use empty materialization store that returns no variants
	inMemoryStore := newInMemoryMaterializationStore(nil)
	defer inMemoryStore.Close()
	// Pre-populate store with variant assignment for the test user
	inMemoryStore.Write(context.Background(), []WriteOp{newWriteOpVariant("experiment_v1", "test-user-123", "flags/sticky-test-flag/rules/sticky-rule", "flags/sticky-test-flag/variants/on")})
	mockedResolver := &tu.MockedLocalResolver{
		Response: &resolver.ResolveWithStickyResponse{
			ResolveResult: &resolver.ResolveWithStickyResponse_MissingMaterializations_{
				MissingMaterializations: &resolver.ResolveWithStickyResponse_MissingMaterializations{
					Items: []*resolver.ResolveWithStickyResponse_MissingMaterializationItem{
						{
							ReadMaterialization: "experiment_v1",
							Unit:                "test-user-123",
							Rule:                "flags/sticky-test-flag/rules/sticky-rule",
						},
					},
				},
			},
		},
	}

	request := &resolver.ResolveWithStickyRequest{
		ResolveRequest:          tu.CreateTutorialFeatureRequest(),
		MaterializationsPerUnit: make(map[string]*resolver.MaterializationMap),
		FailFastOnSticky:        false,
		NotProcessSticky:        false,
	}
	resolver := newMaterializationSupportedResolver(inMemoryStore, mockedResolver)

	response, err := resolver.ResolveWithSticky(request)
	if response != nil {
		t.Fatal("expected nil response")
	}
	expectedErr := "exceeded maximum retries (5) for handling missing materializations"
	if err == nil || err.Error() != expectedErr {
		t.Fatalf("expected error %q, got %v", expectedErr, err)
	}
}
