package confidence

import (
	"context"
	"fmt"
	"os"
	"strconv"
	"time"

	pb "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	"google.golang.org/grpc/metadata"
)

const (
	defaultMaterializationReadTimeoutSeconds  = 2
	defaultMaterializationWriteTimeoutSeconds = 5
)

// remoteMaterializationStore is a MaterializationStore implementation that stores
// materialization data remotely via gRPC to the Confidence service.
//
// This implementation is useful when you want the Confidence service to manage
// materialization storage server-side rather than maintaining local state.
type remoteMaterializationStore struct {
	client       pb.InternalFlagLoggerServiceClient
	clientSecret string
}

// newRemoteMaterializationStore creates a new RemoteMaterializationStore with the given gRPC client.
func newRemoteMaterializationStore(client pb.InternalFlagLoggerServiceClient, clientSecret string) *remoteMaterializationStore {
	return &remoteMaterializationStore{
		client:       client,
		clientSecret: clientSecret,
	}
}

// getMaterializationReadTimeoutSeconds gets the materialization read timeout from environment or returns default
func getMaterializationReadTimeoutSeconds() time.Duration {
	if envVal := os.Getenv("CONFIDENCE_MATERIALIZATION_READ_TIMEOUT_SECONDS"); envVal != "" {
		if seconds, err := strconv.ParseInt(envVal, 10, 64); err == nil {
			return time.Duration(seconds) * time.Second
		}
	}
	return time.Duration(defaultMaterializationReadTimeoutSeconds) * time.Second
}

// getMaterializationWriteTimeoutSeconds gets the materialization write timeout from environment or returns default
func getMaterializationWriteTimeoutSeconds() time.Duration {
	if envVal := os.Getenv("CONFIDENCE_MATERIALIZATION_WRITE_TIMEOUT_SECONDS"); envVal != "" {
		if seconds, err := strconv.ParseInt(envVal, 10, 64); err == nil {
			return time.Duration(seconds) * time.Second
		}
	}
	return time.Duration(defaultMaterializationWriteTimeoutSeconds) * time.Second
}

// Read performs a batch read of materialization data from the remote service.
func (r *remoteMaterializationStore) Read(ctx context.Context, ops []ReadOp) ([]ReadResult, error) {
	if len(ops) == 0 {
		return []ReadResult{}, nil
	}

	// Add deadline to context if not already present
	if _, hasDeadline := ctx.Deadline(); !hasDeadline {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, getMaterializationReadTimeoutSeconds())
		defer cancel()
	}

	// Add authorization header with client secret
	md := metadata.Pairs("authorization", fmt.Sprintf("ClientSecret %s", r.clientSecret))
	ctx = metadata.NewOutgoingContext(ctx, md)

	// Convert ReadOps to proto format
	protoOps := make([]*pb.ReadOp, 0, len(ops))
	for _, op := range ops {
		protoOp, err := readOpToProto(op)
		if err != nil {
			return nil, fmt.Errorf("failed to convert read op: %w", err)
		}
		protoOps = append(protoOps, protoOp)
	}

	// Call gRPC service
	req := &pb.ReadOperationsRequest{
		Ops: protoOps,
	}

	resp, err := r.client.ReadMaterializedOperations(ctx, req)
	if err != nil {
		return nil, fmt.Errorf("failed to read materialized operations: %w", err)
	}

	// Convert proto results to Go types
	results := make([]ReadResult, 0, len(resp.Results))
	for _, protoResult := range resp.Results {
		result, err := protoToReadResult(protoResult)
		if err != nil {
			return nil, fmt.Errorf("failed to convert read result: %w", err)
		}
		results = append(results, result)
	}

	return results, nil
}

// Write performs a batch write of materialization data to the remote service.
func (r *remoteMaterializationStore) Write(ctx context.Context, ops []WriteOp) error {
	if len(ops) == 0 {
		return nil
	}

	// Add deadline to context if not already present
	if _, hasDeadline := ctx.Deadline(); !hasDeadline {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, getMaterializationWriteTimeoutSeconds())
		defer cancel()
	}

	// Add authorization header with client secret
	md := metadata.Pairs("authorization", fmt.Sprintf("ClientSecret %s", r.clientSecret))
	ctx = metadata.NewOutgoingContext(ctx, md)

	// Convert WriteOps to proto format
	protoOps := make([]*pb.VariantData, 0, len(ops))
	for _, op := range ops {
		protoOp, err := writeOpToProto(op)
		if err != nil {
			return fmt.Errorf("failed to convert write op: %w", err)
		}
		protoOps = append(protoOps, protoOp)
	}

	// Call gRPC service
	req := &pb.WriteOperationsRequest{
		StoreVariantOp: protoOps,
	}

	_, err := r.client.WriteMaterializedOperations(ctx, req)
	if err != nil {
		return fmt.Errorf("failed to write materialized operations: %w", err)
	}

	return nil
}

// readOpToProto converts a Go ReadOp interface to a proto ReadOp
func readOpToProto(op ReadOp) (*pb.ReadOp, error) {
	switch v := op.(type) {
	case *ReadOpVariant:
		return &pb.ReadOp{
			Op: &pb.ReadOp_VariantReadOp{
				VariantReadOp: &pb.VariantReadOp{
					Unit:            v.Unit(),
					Materialization: v.Materialization(),
					Rule:            v.Rule(),
				},
			},
		}, nil
	case *ReadOpInclusion:
		return &pb.ReadOp{
			Op: &pb.ReadOp_InclusionReadOp{
				InclusionReadOp: &pb.InclusionReadOp{
					Unit:            v.Unit(),
					Materialization: v.Materialization(),
				},
			},
		}, nil
	default:
		return nil, fmt.Errorf("unknown read op type: %T", op)
	}
}

// protoToReadResult converts a proto ReadResult to a Go ReadResult interface
func protoToReadResult(protoResult *pb.ReadResult) (ReadResult, error) {
	if protoResult == nil {
		return nil, fmt.Errorf("read result is nil")
	}

	// Access the result field which contains the oneof
	if protoResult.Result == nil {
		return nil, fmt.Errorf("read result.result is nil")
	}

	switch v := protoResult.Result.(type) {
	case *pb.ReadResult_VariantResult:
		if v.VariantResult == nil {
			return nil, fmt.Errorf("variant result is nil")
		}
		variant := v.VariantResult.Variant
		return &ReadResultVariant{
			materialization: v.VariantResult.Materialization,
			unit:            v.VariantResult.Unit,
			rule:            v.VariantResult.Rule,
			variant:         &variant,
		}, nil
	case *pb.ReadResult_InclusionResult:
		if v.InclusionResult == nil {
			return nil, fmt.Errorf("inclusion result is nil")
		}
		return &ReadResultInclusion{
			materialization: v.InclusionResult.Materialization,
			unit:            v.InclusionResult.Unit,
			included:        v.InclusionResult.IsIncluded,
		}, nil
	default:
		return nil, fmt.Errorf("unknown read result type: %T", protoResult.Result)
	}
}

// writeOpToProto converts a Go WriteOp interface to a proto VariantData
func writeOpToProto(op WriteOp) (*pb.VariantData, error) {
	switch v := op.(type) {
	case *WriteOpVariant:
		return &pb.VariantData{
			Unit:            v.Unit(),
			Materialization: v.Materialization(),
			Rule:            v.Rule(),
			Variant:         v.Variant(),
		}, nil
	default:
		return nil, fmt.Errorf("unknown write op type: %T", op)
	}
}
