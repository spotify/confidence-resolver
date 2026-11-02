package testutil

import (
	"context"
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	adminv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/admin"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	typesv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/types"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
	gproto "google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/structpb"
)

var repoRoot string

// TestClientSecret is the credential baked into wasm/resolver_state.pb (the
// tutorial fixture). Unit tests that resolve tutorial-feature must use this
// so WASM matches the fixture. It is not a live environment secret.
const TestClientSecret = "mkjJruAATQWjeY7foFIWfVAcBWnci2YF"

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

func (m *MockFlagLogger) RecordEventBatch(publishedCount, rejectedCount int, succeeded bool) {}

type StateProviderMock struct {
	AccountID string
	State     []byte
	Err       error
}

func (m *StateProviderMock) Provide(_ context.Context) ([]byte, string, error) {
	return m.State, m.AccountID, m.Err
}

// requireDataFile reads a test fixture from data/ at the repo root and fails
// the test when it is missing: a silently skipped test is a false green.
// Docker test stages must `COPY data/ /data/` for these to resolve, because
// repoRoot (five directories up from this file) is / inside the flattened
// go test stage.
func requireDataFile(t *testing.T, relPath string) []byte {
	t.Helper()
	dataPath := filepath.Join(repoRoot, relPath)
	data, err := os.ReadFile(dataPath)
	if err != nil {
		t.Fatalf("test fixture %s could not be loaded: %v", dataPath, err)
	}
	return data
}

func LoadTestResolverState(t *testing.T) []byte {
	return requireDataFile(t, "data/resolver_state_current.pb")
}

func LoadTestAccountID(t *testing.T) string {
	return strings.TrimSpace(string(requireDataFile(t, "data/account_id")))
}

func LoadTestEncryptedState(t *testing.T) []byte {
	return requireDataFile(t, "data/resolver_state_encrypted.pb")
}

func LoadTestEncryptionKey(t *testing.T) []byte {
	hexKey := requireDataFile(t, "data/encryption_key_test.hex")
	key, err := hex.DecodeString(strings.TrimSpace(string(hexKey)))
	if err != nil {
		t.Fatalf("Invalid hex encryption key: %v", err)
	}
	return key
}

// CreateStateWithMaterializedSegment creates a test state with a flag using MaterializedSegmentCriterion
func CreateStateWithMaterializedSegment() []byte {
	clientName := "clients/test-client"
	credentialName := "clients/test-client/credentials/test-credential"

	// Create a segment with MaterializedSegmentCriterion
	segment := &adminv1.Segment{
		Name: "segments/custom-targeting-segment",
		Targeting: &typesv1.Targeting{
			Criteria: map[string]*typesv1.Targeting_Criterion{
				"mat-criterion": {
					Criterion: &typesv1.Targeting_Criterion_MaterializedSegment{
						MaterializedSegment: &typesv1.Targeting_Criterion_MaterializedSegmentCriterion{
							MaterializedSegment: "materializedSegments/nicklas-custom-targeting",
						},
					},
				},
			},
			Expression: &typesv1.Expression{
				Expression: &typesv1.Expression_Ref{
					Ref: "mat-criterion",
				},
			},
		},
	}

	// Create a default segment for the fallback rule
	defaultSegment := &adminv1.Segment{
		Name: "segments/always-true",
	}

	// Build bitsets for each segment
	bitsets := []*adminv1.ResolverState_PackedBitset{
		{
			Segment: "segments/custom-targeting-segment",
			Bitset: &adminv1.ResolverState_PackedBitset_FullBitset{
				FullBitset: true,
			},
		},
		{
			Segment: "segments/always-true",
			Bitset: &adminv1.ResolverState_PackedBitset_FullBitset{
				FullBitset: true,
			},
		},
	}

	state := &adminv1.ResolverState{
		Flags: []*adminv1.Flag{
			{
				Name: "flags/custom-targeted-flag",
				Variants: []*adminv1.Flag_Variant{
					{
						Name: "flags/custom-targeted-flag/variants/cake-exclamation",
						Value: &structpb.Struct{
							Fields: map[string]*structpb.Value{
								"message": structpb.NewStringValue("Did someone say CAKE?!"),
							},
						},
					},
					{
						Name: "flags/custom-targeted-flag/variants/default",
						Value: &structpb.Struct{
							Fields: map[string]*structpb.Value{
								"message": structpb.NewStringValue("nothing fun"),
							},
						},
					},
				},
				State:   adminv1.Flag_ACTIVE,
				Clients: []string{clientName},
				Rules: []*adminv1.Flag_Rule{
					{
						Name:                 "flags/custom-targeted-flag/rules/custom-targeting",
						Segment:              segment.Name,
						TargetingKeySelector: "user_id",
						Enabled:              true,
						AssignmentSpec: &adminv1.Flag_Rule_AssignmentSpec{
							BucketCount: 2,
							Assignments: []*adminv1.Flag_Rule_Assignment{
								{
									AssignmentId: "variant-assignment",
									Assignment: &adminv1.Flag_Rule_Assignment_Variant{
										Variant: &adminv1.Flag_Rule_Assignment_VariantAssignment{
											Variant: "flags/custom-targeted-flag/variants/cake-exclamation",
										},
									},
									BucketRanges: []*adminv1.Flag_Rule_BucketRange{
										{
											Lower: 0,
											Upper: 2,
										},
									},
								},
							},
						},
					},
					{
						Name:                 "flags/custom-targeted-flag/rules/default-rule",
						Segment:              defaultSegment.Name,
						TargetingKeySelector: "user_id",
						Enabled:              true,
						AssignmentSpec: &adminv1.Flag_Rule_AssignmentSpec{
							BucketCount: 2,
							Assignments: []*adminv1.Flag_Rule_Assignment{
								{
									AssignmentId: "default-assignment",
									Assignment: &adminv1.Flag_Rule_Assignment_Variant{
										Variant: &adminv1.Flag_Rule_Assignment_VariantAssignment{
											Variant: "flags/custom-targeted-flag/variants/default",
										},
									},
									BucketRanges: []*adminv1.Flag_Rule_BucketRange{
										{
											Lower: 0,
											Upper: 2,
										},
									},
								},
							},
						},
					},
				},
			},
		},
		SegmentsNoBitsets: []*adminv1.Segment{segment, defaultSegment},
		Clients: []*adminv1.Client{
			{
				Name: clientName,
			},
		},
		Bitsets: bitsets,
		ClientCredentials: []*adminv1.ClientCredential{
			{
				Name: credentialName,
				Credential: &adminv1.ClientCredential_ClientSecret_{
					ClientSecret: &adminv1.ClientCredential_ClientSecret{
						Secret: "test-secret",
					},
				},
			},
		},
	}

	data, err := gproto.Marshal(state)
	if err != nil {
		panic("Failed to create state with materialized segment: " + err.Error())
	}
	return data
}

// Helper function to create minimal valid resolver state for testing
func CreateMinimalResolverState() []byte {
	clientName := "clients/test-client"
	credentialName := "clients/test-client/credentials/test-credential"

	state := &adminv1.ResolverState{
		Flags: []*adminv1.Flag{},
		Clients: []*adminv1.Client{
			{
				Name: clientName,
			},
		},
		ClientCredentials: []*adminv1.ClientCredential{
			{
				Name: credentialName, // Must start with client name
				Credential: &adminv1.ClientCredential_ClientSecret_{
					ClientSecret: &adminv1.ClientCredential_ClientSecret{
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
	segments := []*adminv1.Segment{
		{
			Name: "segments/always-true",
		},
	}

	// Build bitsets for each segment
	bitsets := make([]*adminv1.ResolverState_PackedBitset, 0, len(segments))
	for _, segment := range segments {
		bitsets = append(bitsets, &adminv1.ResolverState_PackedBitset{
			Segment: segment.Name,
			Bitset: &adminv1.ResolverState_PackedBitset_FullBitset{
				FullBitset: true,
			},
		})
	}
	state := &adminv1.ResolverState{
		Flags: []*adminv1.Flag{
			{
				Name: "flags/sticky-test-flag",
				Variants: []*adminv1.Flag_Variant{
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
				State: adminv1.Flag_ACTIVE,
				// Associate this flag with the test client
				Clients: []string{"clients/test-client"},
				Rules: []*adminv1.Flag_Rule{
					{
						Name:                 "flags/sticky-test-flag/rules/sticky-rule",
						Segment:              segments[0].Name,
						TargetingKeySelector: "user_id",
						Enabled:              true,
						AssignmentSpec: &adminv1.Flag_Rule_AssignmentSpec{
							BucketCount: 2,
							Assignments: []*adminv1.Flag_Rule_Assignment{
								{
									AssignmentId: "variant-assignment",
									Assignment: &adminv1.Flag_Rule_Assignment_Variant{
										Variant: &adminv1.Flag_Rule_Assignment_VariantAssignment{
											Variant: "flags/sticky-test-flag/variants/on",
										},
									},
									BucketRanges: []*adminv1.Flag_Rule_BucketRange{
										{
											Lower: 0,
											Upper: 2,
										},
									},
								},
							},
						},
						// This rule requires a materialization named "experiment_v1"
						MaterializationSpec: &adminv1.Flag_Rule_MaterializationSpec{
							ReadMaterialization:  "experiment_v1",
							WriteMaterialization: "experiment_v1",
							Mode: &adminv1.Flag_Rule_MaterializationSpec_MaterializationReadMode{
								MaterializationMustMatch:     false,
								SegmentTargetingCanBeIgnored: false,
							},
						},
					},
				},
			},
		},
		SegmentsNoBitsets: segments,
		Clients: []*adminv1.Client{
			{
				Name: "clients/test-client",
			},
		},
		// All-one bitset for each segment
		Bitsets: bitsets,
		ClientCredentials: []*adminv1.ClientCredential{
			{
				// ClientCredential name must start with the client name
				Name: "clients/test-client/credentials/test-credential",
				Credential: &adminv1.ClientCredential_ClientSecret_{
					ClientSecret: &adminv1.ClientCredential_ClientSecret{
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

// CreateResolveProcessRequest creates a ResolveProcessRequest with DeferredMaterializations.
func CreateResolveProcessRequest(resolveRequest *resolver.ResolveFlagsRequest) *wasm.ResolveProcessRequest {
	return &wasm.ResolveProcessRequest{
		Resolve: &wasm.ResolveProcessRequest_DeferredMaterializations{
			DeferredMaterializations: resolveRequest,
		},
	}
}

// Helper function to create a tutorial-feature resolve request with standard test data
func CreateTutorialFeatureRequest() *resolver.ResolveFlagsRequest {
	return &resolver.ResolveFlagsRequest{
		Flags:        []string{"flags/tutorial-feature"},
		Apply:        true,
		ClientSecret: TestClientSecret,
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
			},
		},
		ResolveId: "test-resolve-id",
	}
}

// MockedLocalResolver is a test double implementing the LocalResolver API used in tests.
type MockedLocalResolver struct {
	// Single response fallback
	Response             *wasm.ResolveProcessResponse
	Err                  error
	LastRequest          *wasm.ResolveProcessRequest
	LastSetResolverState *wasm.SetResolverStateRequest
	// Sequenced responses support
	Responses []*wasm.ResolveProcessResponse
	callIdx   int
}

func (m MockedLocalResolver) Close(context.Context) error { return nil }
func (m MockedLocalResolver) FlushAllLogs() error         { return nil }
func (m MockedLocalResolver) FlushAssignLogs() error      { return nil }
func (m *MockedLocalResolver) ResolveProcess(req *wasm.ResolveProcessRequest) (*wasm.ResolveProcessResponse, error) {
	m.LastRequest = req
	if len(m.Responses) > 0 {
		idx := m.callIdx
		if idx >= len(m.Responses) {
			return m.Responses[len(m.Responses)-1], m.Err
		}
		resp := m.Responses[idx]
		m.callIdx++
		return resp, m.Err
	}
	return m.Response, m.Err
}
func (m *MockedLocalResolver) SetResolverState(req *wasm.SetResolverStateRequest) error {
	m.LastSetResolverState = req
	return nil
}
func (m MockedLocalResolver) PrometheusSnapshot(_ uint32, _ bool) string   { return "" }
func (m MockedLocalResolver) RegisterResolve(*wasm.RegisterResolveRequest) {}
func (m MockedLocalResolver) ApplyFlags(*resolver.ApplyFlagsRequest) error { return nil }

func MustJSONToProto(jsonString string) *structpb.Value {
	var v structpb.Value
	err := json.Unmarshal([]byte(jsonString), &v)
	if err != nil {
		panic(err)
	}
	return &v
}
