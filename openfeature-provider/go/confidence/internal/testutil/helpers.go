package testutil

import (
	"context"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	admin "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/admin"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
	gproto "google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/structpb"
)

var repoRoot string

func init() {
	// Resolve paths relative to this source file to avoid dependence on cwd.
	if _, thisFile, _, ok := runtime.Caller(0); ok {
		// helpers.go lives at: openfeature-provider/go/confidence/internal/testutil/helpers.go
		// repo root is five directories up from this file
		repoRoot = filepath.Join(filepath.Dir(thisFile), "..", "..", "..", "..", "..")
	} else {
		panic("failed to resolve test repo root via runtime.Caller")
	}
}

type MockFlagLogger struct {
	writeFunc    func(request *resolverv1.WriteFlagLogsRequest)
	shutdownFunc func()
}

func (m *MockFlagLogger) Shutdown() {
	if m.shutdownFunc != nil {
		m.shutdownFunc()
	}
}

func (m *MockFlagLogger) Write(request *resolverv1.WriteFlagLogsRequest) {
	m.writeFunc(request)
}

type StateProviderMock struct {
	AccountID string
	State     []byte
	Err       error
}

func (m *StateProviderMock) Provide(_ context.Context) ([]byte, string, error) {
	return m.State, m.AccountID, m.Err
}

func LoadTestResolverState(t *testing.T) []byte {
	dataPath := filepath.Join(repoRoot, "data", "resolver_state_current.pb")
	data, err := os.ReadFile(dataPath)
	if err != nil {
		t.Skipf("Skipping test - could not load test resolver state: %v", err)
	}
	return data
}

func LoadTestAccountID(t *testing.T) string {
	dataPath := filepath.Join(repoRoot, "data", "account_id")
	data, err := os.ReadFile(dataPath)
	if err != nil {
		t.Skipf("Skipping test - could not load test account ID: %v", err)
	}
	return strings.TrimSpace(string(data))
}

// Helper function to create minimal valid resolver state for testing
func CreateMinimalResolverState() []byte {
	clientName := "clients/test-client"
	credentialName := "clients/test-client/credentials/test-credential"

	state := &admin.ResolverState{
		Flags: []*admin.Flag{},
		Clients: []*admin.Client{
			{
				Name: clientName,
			},
		},
		ClientCredentials: []*admin.ClientCredential{
			{
				Name: credentialName, // Must start with client name
				Credential: &admin.ClientCredential_ClientSecret_{
					ClientSecret: &admin.ClientCredential_ClientSecret{
						Secret: "test-secret",
					},
				},
			},
		},
	}
	data, err := gproto.Marshal(state)
	if err != nil {
		panic("Failed to create minimal state: " + err.Error())
	}
	return data
}

// Helper to create a resolver state with a flag that requires materializations
func CreateStateWithStickyFlag() []byte {
	segments := []*admin.Segment{
		{
			Name: "segments/always-true",
		},
	}

	// Build bitsets for each segment
	bitsets := make([]*admin.ResolverState_PackedBitset, 0, len(segments))
	for _, segment := range segments {
		bitsets = append(bitsets, &admin.ResolverState_PackedBitset{
			Segment: segment.Name,
			Bitset: &admin.ResolverState_PackedBitset_FullBitset{
				FullBitset: true,
			},
		})
	}
	state := &admin.ResolverState{
		Flags: []*admin.Flag{
			{
				Name: "flags/sticky-test-flag",
				Variants: []*admin.Flag_Variant{
					{
						Name: "flags/sticky-test-flag/variants/on",
						Value: &structpb.Struct{
							Fields: map[string]*structpb.Value{
								"enabled": structpb.NewBoolValue(true),
							},
						},
					},
					{
						Name: "flags/sticky-test-flag/variants/off",
						Value: &structpb.Struct{
							Fields: map[string]*structpb.Value{
								"enabled": structpb.NewBoolValue(false),
							},
						},
					},
				},
				State: admin.Flag_ACTIVE,
				// Associate this flag with the test client
				Clients: []string{"clients/test-client"},
				Rules: []*admin.Flag_Rule{
					{
						Name:                 "flags/sticky-test-flag/rules/sticky-rule",
						Segment:              segments[0].Name,
						TargetingKeySelector: "user_id",
						Enabled:              true,
						AssignmentSpec: &admin.Flag_Rule_AssignmentSpec{
							BucketCount: 2,
							Assignments: []*admin.Flag_Rule_Assignment{
								{
									AssignmentId: "variant-assignment",
									Assignment: &admin.Flag_Rule_Assignment_Variant{
										Variant: &admin.Flag_Rule_Assignment_VariantAssignment{
											Variant: "flags/sticky-test-flag/variants/on",
										},
									},
									BucketRanges: []*admin.Flag_Rule_BucketRange{
										{
											Lower: 0,
											Upper: 2,
										},
									},
								},
							},
						},
						// This rule requires a materialization named "experiment_v1"
						MaterializationSpec: &admin.Flag_Rule_MaterializationSpec{
							ReadMaterialization:  "experiment_v1",
							WriteMaterialization: "experiment_v1",
							Mode: &admin.Flag_Rule_MaterializationSpec_MaterializationReadMode{
								MaterializationMustMatch:     false,
								SegmentTargetingCanBeIgnored: false,
							},
						},
					},
				},
			},
		},
		SegmentsNoBitsets: segments,
		Clients: []*admin.Client{
			{
				Name: "clients/test-client",
			},
		},
		// All-one bitset for each segment
		Bitsets: bitsets,
		ClientCredentials: []*admin.ClientCredential{
			{
				// ClientCredential name must start with the client name
				Name: "clients/test-client/credentials/test-credential",
				Credential: &admin.ClientCredential_ClientSecret_{
					ClientSecret: &admin.ClientCredential_ClientSecret{
						Secret: "test-secret",
					},
				},
			},
		},
	}
	data, err := gproto.Marshal(state)
	if err != nil {
		panic("Failed to create state with sticky flag: " + err.Error())
	}
	return data
}

// Helper function to create a ResolveWithStickyRequest
func CreateResolveWithStickyRequest(
	resolveRequest *resolver.ResolveFlagsRequest,
	materializations map[string]*wasm.MaterializationMap,
	failFast bool,
	notProcessSticky bool,
) *wasm.ResolveWithStickyRequest {
	if materializations == nil {
		materializations = make(map[string]*wasm.MaterializationMap)
	}
	return &wasm.ResolveWithStickyRequest{
		ResolveRequest:          resolveRequest,
		MaterializationsPerUnit: materializations,
		FailFastOnSticky:        failFast,
		NotProcessSticky:        notProcessSticky,
	}
}

// Helper function to create a tutorial-feature resolve request with standard test data
func CreateTutorialFeatureRequest() *resolver.ResolveFlagsRequest {
	return &resolver.ResolveFlagsRequest{
		Flags:        []string{"flags/tutorial-feature"},
		Apply:        true,
		ClientSecret: "mkjJruAATQWjeY7foFIWfVAcBWnci2YF",
		EvaluationContext: &structpb.Struct{
			Fields: map[string]*structpb.Value{
				"visitor_id": structpb.NewStringValue("tutorial_visitor"),
			},
		},
	}
}

// Helper function to create a response matching CreateTutorialFeatureRequest
func CreateTutorialFeatureResponse() *resolver.ResolveFlagsResponse {
	return &resolver.ResolveFlagsResponse{
		ResolvedFlags: []*resolver.ResolvedFlag{
			{
				Flag:    "flags/tutorial-feature",
				Variant: "flags/tutorial-feature/variants/on",
				Value: &structpb.Struct{Fields: map[string]*structpb.Value{
					"enabled": structpb.NewBoolValue(true),
				}},
				ShouldApply: true,
			},
		},
		ResolveId: "test-resolve-id",
	}
}

// MockedLocalResolver is a test double implementing the LocalResolver API used in tests.
type MockedLocalResolver struct {
	// Single response fallback
	Response *wasm.ResolveWithStickyResponse
	Err      error
	// Sequenced responses support
	Responses []*wasm.ResolveWithStickyResponse
	callIdx   int
}

func (m MockedLocalResolver) Close(context.Context) error { return nil }
func (m MockedLocalResolver) FlushAllLogs() error         { return nil }
func (m MockedLocalResolver) FlushAssignLogs() error      { return nil }
func (m *MockedLocalResolver) ResolveWithSticky(*wasm.ResolveWithStickyRequest) (*wasm.ResolveWithStickyResponse, error) {
	if len(m.Responses) > 0 {
		idx := m.callIdx
		if idx >= len(m.Responses) {
			// If calls exceed provided responses, return last response
			return m.Responses[len(m.Responses)-1], m.Err
		}
		resp := m.Responses[idx]
		m.callIdx++
		return resp, m.Err
	}
	return m.Response, m.Err
}
func (m MockedLocalResolver) SetResolverState(*wasm.SetResolverStateRequest) error { return nil }
