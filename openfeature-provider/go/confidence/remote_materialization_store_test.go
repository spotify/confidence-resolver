package confidence

import (
	"context"
	"errors"
	"testing"
	"time"

	pb "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
	tu "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"
)

// mockInternalFlagLoggerServiceClient is a mock implementation for testing remote materialization store
type mockInternalFlagLoggerServiceClient struct {
	pb.InternalFlagLoggerServiceClient
	readMaterializedOperationsFunc  func(ctx context.Context, req *pb.ReadOperationsRequest) (*pb.ReadOperationsResult, error)
	writeMaterializedOperationsFunc func(ctx context.Context, req *pb.WriteOperationsRequest) (*pb.WriteOperationsResult, error)
}

func (m *mockInternalFlagLoggerServiceClient) ReadMaterializedOperations(ctx context.Context, req *pb.ReadOperationsRequest, opts ...grpc.CallOption) (*pb.ReadOperationsResult, error) {
	if m.readMaterializedOperationsFunc != nil {
		return m.readMaterializedOperationsFunc(ctx, req)
	}
	return &pb.ReadOperationsResult{}, nil
}

func (m *mockInternalFlagLoggerServiceClient) WriteMaterializedOperations(ctx context.Context, req *pb.WriteOperationsRequest, opts ...grpc.CallOption) (*pb.WriteOperationsResult, error) {
	if m.writeMaterializedOperationsFunc != nil {
		return m.writeMaterializedOperationsFunc(ctx, req)
	}
	return &pb.WriteOperationsResult{}, nil
}

func TestRemoteMaterializationStore_Read_DeadlineExceeded(t *testing.T) {
	mockClient := &mockInternalFlagLoggerServiceClient{
		readMaterializedOperationsFunc: func(ctx context.Context, req *pb.ReadOperationsRequest) (*pb.ReadOperationsResult, error) {
			return nil, status.Error(codes.DeadlineExceeded, "deadline exceeded")
		},
	}

	store := newRemoteMaterializationStore(mockClient, "test-secret")
	readOps := []ReadOp{
		newReadOpVariant("exp_v1", "user-123", "rule-1"),
	}

	_, err := store.Read(context.Background(), readOps)
	if err == nil {
		t.Fatal("expected error, got nil")
	}

	expectedErrMsg := "failed to read materialized operations: rpc error: code = DeadlineExceeded desc = deadline exceeded"
	if err.Error() != expectedErrMsg {
		t.Errorf("expected error message %q, got %q", expectedErrMsg, err.Error())
	}
}

func TestRemoteMaterializationStore_Read_Unauthenticated(t *testing.T) {
	mockClient := &mockInternalFlagLoggerServiceClient{
		readMaterializedOperationsFunc: func(ctx context.Context, req *pb.ReadOperationsRequest) (*pb.ReadOperationsResult, error) {
			return nil, status.Error(codes.Unauthenticated, "invalid credentials")
		},
	}

	store := newRemoteMaterializationStore(mockClient, "test-secret")
	readOps := []ReadOp{
		newReadOpVariant("exp_v1", "user-123", "rule-1"),
	}

	_, err := store.Read(context.Background(), readOps)
	if err == nil {
		t.Fatal("expected error, got nil")
	}

	expectedErrMsg := "failed to read materialized operations: rpc error: code = Unauthenticated desc = invalid credentials"
	if err.Error() != expectedErrMsg {
		t.Errorf("expected error message %q, got %q", expectedErrMsg, err.Error())
	}
}

func TestRemoteMaterializationStore_Read_Unavailable(t *testing.T) {
	mockClient := &mockInternalFlagLoggerServiceClient{
		readMaterializedOperationsFunc: func(ctx context.Context, req *pb.ReadOperationsRequest) (*pb.ReadOperationsResult, error) {
			return nil, status.Error(codes.Unavailable, "service unavailable")
		},
	}

	store := newRemoteMaterializationStore(mockClient, "test-secret")
	readOps := []ReadOp{
		newReadOpVariant("exp_v1", "user-123", "rule-1"),
	}

	_, err := store.Read(context.Background(), readOps)
	if err == nil {
		t.Fatal("expected error, got nil")
	}

	expectedErrMsg := "failed to read materialized operations: rpc error: code = Unavailable desc = service unavailable"
	if err.Error() != expectedErrMsg {
		t.Errorf("expected error message %q, got %q", expectedErrMsg, err.Error())
	}
}

func TestRemoteMaterializationStore_Read_GenericError(t *testing.T) {
	mockClient := &mockInternalFlagLoggerServiceClient{
		readMaterializedOperationsFunc: func(ctx context.Context, req *pb.ReadOperationsRequest) (*pb.ReadOperationsResult, error) {
			return nil, errors.New("network error")
		},
	}

	store := newRemoteMaterializationStore(mockClient, "test-secret")
	readOps := []ReadOp{
		newReadOpVariant("exp_v1", "user-123", "rule-1"),
	}

	_, err := store.Read(context.Background(), readOps)
	if err == nil {
		t.Fatal("expected error, got nil")
	}

	expectedErrMsg := "failed to read materialized operations: network error"
	if err.Error() != expectedErrMsg {
		t.Errorf("expected error message %q, got %q", expectedErrMsg, err.Error())
	}
}

func TestRemoteMaterializationStore_Write_DeadlineExceeded(t *testing.T) {
	mockClient := &mockInternalFlagLoggerServiceClient{
		writeMaterializedOperationsFunc: func(ctx context.Context, req *pb.WriteOperationsRequest) (*pb.WriteOperationsResult, error) {
			return nil, status.Error(codes.DeadlineExceeded, "deadline exceeded")
		},
	}

	store := newRemoteMaterializationStore(mockClient, "test-secret")
	writeOps := []WriteOp{
		newWriteOpVariant("exp_v1", "user-123", "rule-1", "variant-a"),
	}

	err := store.Write(context.Background(), writeOps)
	if err == nil {
		t.Fatal("expected error, got nil")
	}

	expectedErrMsg := "failed to write materialized operations: rpc error: code = DeadlineExceeded desc = deadline exceeded"
	if err.Error() != expectedErrMsg {
		t.Errorf("expected error message %q, got %q", expectedErrMsg, err.Error())
	}
}

func TestRemoteMaterializationStore_Write_PermissionDenied(t *testing.T) {
	mockClient := &mockInternalFlagLoggerServiceClient{
		writeMaterializedOperationsFunc: func(ctx context.Context, req *pb.WriteOperationsRequest) (*pb.WriteOperationsResult, error) {
			return nil, status.Error(codes.PermissionDenied, "permission denied")
		},
	}

	store := newRemoteMaterializationStore(mockClient, "test-secret")
	writeOps := []WriteOp{
		newWriteOpVariant("exp_v1", "user-123", "rule-1", "variant-a"),
	}

	err := store.Write(context.Background(), writeOps)
	if err == nil {
		t.Fatal("expected error, got nil")
	}

	expectedErrMsg := "failed to write materialized operations: rpc error: code = PermissionDenied desc = permission denied"
	if err.Error() != expectedErrMsg {
		t.Errorf("expected error message %q, got %q", expectedErrMsg, err.Error())
	}
}

func TestRemoteMaterializationStore_Read_ContextTimeout(t *testing.T) {
	mockClient := &mockInternalFlagLoggerServiceClient{
		readMaterializedOperationsFunc: func(ctx context.Context, req *pb.ReadOperationsRequest) (*pb.ReadOperationsResult, error) {
			// Verify that context has a deadline
			deadline, ok := ctx.Deadline()
			if !ok {
				t.Error("expected context to have deadline")
			}
			if time.Until(deadline) > 3*time.Second {
				t.Errorf("deadline should be ~2 seconds (default read timeout), got %v", time.Until(deadline))
			}
			return &pb.ReadOperationsResult{}, nil
		},
	}

	store := newRemoteMaterializationStore(mockClient, "test-secret")
	readOps := []ReadOp{
		newReadOpVariant("exp_v1", "user-123", "rule-1"),
	}

	_, err := store.Read(context.Background(), readOps)
	if err != nil {
		t.Fatalf("expected no error, got %v", err)
	}
}

func TestRemoteMaterializationStore_Write_ContextTimeout(t *testing.T) {
	mockClient := &mockInternalFlagLoggerServiceClient{
		writeMaterializedOperationsFunc: func(ctx context.Context, req *pb.WriteOperationsRequest) (*pb.WriteOperationsResult, error) {
			// Verify that context has a deadline
			deadline, ok := ctx.Deadline()
			if !ok {
				t.Error("expected context to have deadline")
			}
			if time.Until(deadline) > 6*time.Second {
				t.Errorf("deadline should be ~5 seconds (default write timeout), got %v", time.Until(deadline))
			}
			return &pb.WriteOperationsResult{}, nil
		},
	}

	store := newRemoteMaterializationStore(mockClient, "test-secret")
	writeOps := []WriteOp{
		newWriteOpVariant("exp_v1", "user-123", "rule-1", "variant-a"),
	}

	err := store.Write(context.Background(), writeOps)
	if err != nil {
		t.Fatalf("expected no error, got %v", err)
	}
}

// TestRemoteMaterializationStore_ErrorPropagation_InProvider tests that errors from
// the remote materialization store are properly propagated through the provider
func TestRemoteMaterializationStore_ErrorPropagation_InProvider(t *testing.T) {
	// Create a mock client that returns an error on read
	mockClient := &mockInternalFlagLoggerServiceClient{
		readMaterializedOperationsFunc: func(ctx context.Context, req *pb.ReadOperationsRequest) (*pb.ReadOperationsResult, error) {
			return nil, status.Error(codes.Unavailable, "service unavailable")
		},
	}

	// Create remote store
	remoteStore := newRemoteMaterializationStore(mockClient, "test-secret")

	// Create mocked resolver that requests missing materializations
	mockedResolver := &tu.MockedLocalResolver{
		Response: &wasm.ResolveWithStickyResponse{
			ResolveResult: &wasm.ResolveWithStickyResponse_ReadOpsRequest{
				ReadOpsRequest: &pb.ReadOperationsRequest{
					Ops: []*pb.ReadOp{
						{
							Op: &pb.ReadOp_VariantReadOp{
								VariantReadOp: &pb.VariantReadOp{
									Unit:            "test-user-123",
									Materialization: "experiment_v1",
									Rule:            "flags/sticky-test-flag/rules/sticky-rule",
								},
							},
						},
					},
				},
			},
		},
	}

	request := &wasm.ResolveWithStickyRequest{
		ResolveRequest:   tu.CreateTutorialFeatureRequest(),
		Materializations: []*pb.ReadResult{},
		FailFastOnSticky: false,
		NotProcessSticky: false,
	}

	materializationResolver := newMaterializationSupportedResolver(remoteStore, mockedResolver)

	// The error from the remote store should be propagated
	_, err := materializationResolver.ResolveWithSticky(request)
	if err == nil {
		t.Fatal("expected error to be propagated, got nil")
	}

	// Verify the error message contains information about the remote failure
	expectedErrMsg := "failed to handle missing materializations: failed to read materialized operations: rpc error: code = Unavailable desc = service unavailable"
	if err.Error() != expectedErrMsg {
		t.Errorf("expected error message %q, got %q", expectedErrMsg, err.Error())
	}
}

// TestRemoteMaterializationStore_WriteErrorDoesNotBlock tests that write errors
// don't block resolution (since writes are async)
func TestRemoteMaterializationStore_WriteErrorDoesNotBlock(t *testing.T) {
	writeCallCount := 0
	mockClient := &mockInternalFlagLoggerServiceClient{
		writeMaterializedOperationsFunc: func(ctx context.Context, req *pb.WriteOperationsRequest) (*pb.WriteOperationsResult, error) {
			writeCallCount++
			// Return an error
			return nil, status.Error(codes.Internal, "internal error")
		},
	}

	remoteStore := newRemoteMaterializationStore(mockClient, "test-secret")

	mockedResolver := &tu.MockedLocalResolver{
		Response: &wasm.ResolveWithStickyResponse{
			ResolveResult: &wasm.ResolveWithStickyResponse_Success_{
				Success: &wasm.ResolveWithStickyResponse_Success{
					Response: tu.CreateTutorialFeatureResponse(),
					MaterializationUpdates: []*pb.VariantData{
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

	request := &wasm.ResolveWithStickyRequest{
		ResolveRequest:   tu.CreateTutorialFeatureRequest(),
		Materializations: []*pb.ReadResult{},
		FailFastOnSticky: false,
		NotProcessSticky: false,
	}

	materializationResolver := newMaterializationSupportedResolver(remoteStore, mockedResolver)

	// Resolution should succeed even though write will fail
	response, err := materializationResolver.ResolveWithSticky(request)
	if err != nil {
		t.Fatalf("expected no error (write is async), got %v", err)
	}
	if response == nil {
		t.Fatal("expected non-nil response")
	}

	// Wait for async write to complete
	time.Sleep(100 * time.Millisecond)

	// Verify write was attempted (even though it failed)
	if writeCallCount != 1 {
		t.Errorf("expected 1 write attempt, got %d", writeCallCount)
	}
}

// TestRemoteMaterializationStore_SuccessfulReadAndWrite tests the happy path
func TestRemoteMaterializationStore_SuccessfulReadAndWrite(t *testing.T) {
	var storedData = make(map[string]string) // key: unit+materialization+rule, value: variant

	mockClient := &mockInternalFlagLoggerServiceClient{
		readMaterializedOperationsFunc: func(ctx context.Context, req *pb.ReadOperationsRequest) (*pb.ReadOperationsResult, error) {
			results := make([]*pb.ReadResult, 0, len(req.Ops))
			for _, op := range req.Ops {
				if variantOp := op.GetVariantReadOp(); variantOp != nil {
					key := variantOp.Unit + variantOp.Materialization + variantOp.Rule
					variant, exists := storedData[key]

					variantData := &pb.VariantData{
						Unit:            variantOp.Unit,
						Materialization: variantOp.Materialization,
						Rule:            variantOp.Rule,
						Variant:         variant,
					}

					if exists {
						results = append(results, &pb.ReadResult{
							Result: &pb.ReadResult_VariantResult{
								VariantResult: variantData,
							},
						})
					}
				}
			}
			return &pb.ReadOperationsResult{Results: results}, nil
		},
		writeMaterializedOperationsFunc: func(ctx context.Context, req *pb.WriteOperationsRequest) (*pb.WriteOperationsResult, error) {
			for _, variantData := range req.StoreVariantOp {
				key := variantData.Unit + variantData.Materialization + variantData.Rule
				storedData[key] = variantData.Variant
			}
			return &pb.WriteOperationsResult{}, nil
		},
	}

	remoteStore := newRemoteMaterializationStore(mockClient, "test-secret")

	// Write data
	writeOps := []WriteOp{
		newWriteOpVariant("exp_v1", "user-123", "rule-1", "variant-a"),
	}
	err := remoteStore.Write(context.Background(), writeOps)
	if err != nil {
		t.Fatalf("write failed: %v", err)
	}

	// Read it back
	readOps := []ReadOp{
		newReadOpVariant("exp_v1", "user-123", "rule-1"),
	}
	results, err := remoteStore.Read(context.Background(), readOps)
	if err != nil {
		t.Fatalf("read failed: %v", err)
	}

	if len(results) != 1 {
		t.Fatalf("expected 1 result, got %d", len(results))
	}

	variantResult, ok := results[0].(*ReadResultVariant)
	if !ok {
		t.Fatalf("expected ReadResultVariant, got %T", results[0])
	}

	if variantResult.Variant() == nil || *variantResult.Variant() != "variant-a" {
		t.Errorf("expected variant 'variant-a', got %v", variantResult.Variant())
	}
}
