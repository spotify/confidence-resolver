package confidence

import (
	"context"
	"fmt"

	lr "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/local_resolver"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
)

// materializationSupportedResolver wraps a LocalResolver and allows it to read and write materializations.
type materializationSupportedResolver struct {
	store   MaterializationStore
	current lr.LocalResolver
}

func wrapResolverSupplierWithMaterializations(supplier LocalResolverSupplier, materializationStore MaterializationStore) LocalResolverSupplier {
	return func(ctx context.Context, logSink lr.LogSink) lr.LocalResolver {
		localResolver := supplier(ctx, logSink)
		return &materializationSupportedResolver{
			store:   materializationStore,
			current: localResolver,
		}
	}
}

func (m *materializationSupportedResolver) ResolveWithSticky(request *wasm.ResolveWithStickyRequest) (resp *wasm.ResolveWithStickyResponse, err error) {
	response, err := m.current.ResolveWithSticky(request)
	if err != nil {
		return nil, err
	}
	return m.handleStickyResponseWithDepth(request, response, 0)

}

// max number of recursive retries when handling missing materializations
const maxStickyRetryDepth = 5

func (m *materializationSupportedResolver) handleStickyResponseWithDepth(request *wasm.ResolveWithStickyRequest, response *wasm.ResolveWithStickyResponse, depth int) (*wasm.ResolveWithStickyResponse, error) {
	if depth >= maxStickyRetryDepth {
		// Stop retrying to avoid potential infinite recursion
		return nil, fmt.Errorf("exceeded maximum retries (%d) for handling missing materializations", maxStickyRetryDepth)
	}
	switch result := response.ResolveResult.(type) {
	case *wasm.ResolveWithStickyResponse_Success_:
		success := result.Success
		// Store updates if present
		if len(success.GetMaterializationUpdates()) > 0 {
			m.storeUpdates(success.GetMaterializationUpdates())
		}
		return response, nil

	case *wasm.ResolveWithStickyResponse_ReadOpsRequest:
		missingMaterializations := result.ReadOpsRequest
		// Try to load missing materializations from store
		updatedRequest, err := m.handleMissingMaterializations(request, missingMaterializations.GetOps())
		if err != nil {
			return nil, fmt.Errorf("failed to handle missing materializations: %w", err)
		}
		// Retry with the updated request
		retryResponse, err := m.current.ResolveWithSticky(updatedRequest)
		if err != nil {
			return nil, err
		}
		// Recursively handle the response (in case there are more missing materializations)
		return m.handleStickyResponseWithDepth(updatedRequest, retryResponse, depth+1)

	default:
		return nil, fmt.Errorf("unexpected resolve result type: %T", response.ResolveResult)

	}
}

func (m *materializationSupportedResolver) storeUpdates(updates []*resolverinternal.VariantData) {
	// Convert protobuf updates to WriteOp slice
	writeOps := make([]WriteOp, len(updates))
	for i, update := range updates {
		writeOps[i] = newWriteOpVariant(
			update.GetMaterialization(),
			update.GetUnit(),
			update.GetRule(),
			update.GetVariant(),
		)
	}

	// Store updates asynchronously
	go func() {
		if err := m.store.Write(context.Background(), writeOps); err != nil {
			// Check if it's an unsupported operation error (expected for UnsupportedMaterializationStore)
			if _, ok := err.(*MaterializationNotSupportedError); !ok {
				// TODO: Add proper logging
				_ = err
			}
		}
	}()
}

// handleMissingMaterializations loads missing materializations from the store
// and returns an updated request with the materializations added
func (m *materializationSupportedResolver) handleMissingMaterializations(request *wasm.ResolveWithStickyRequest, missingItems []*resolverinternal.ReadOp) (*wasm.ResolveWithStickyRequest, error) {
	// Convert missing items to ReadOp slice
	readOps := make([]ReadOp, len(missingItems))
	for i, item := range missingItems {
		if item.GetInclusionReadOp() != nil {
			readOps[i] = newReadOpInclusion(
				item.GetInclusionReadOp().GetMaterialization(),
				item.GetInclusionReadOp().GetUnit(),
			)
		} else {
			readOps[i] = newReadOpVariant(
				item.GetVariantReadOp().GetMaterialization(),
				item.GetVariantReadOp().GetUnit(),
				item.GetVariantReadOp().GetRule(),
			)
		}
	}

	// Read from the store
	results, err := m.store.Read(context.Background(), readOps)
	if err != nil {
		return nil, err
	}

	// Start with existing materializations
	materializations := make([]*resolverinternal.ReadResult, len(request.GetMaterializations()))
	copy(materializations, request.GetMaterializations())

	for _, result := range results {
		protoResult, err := readResultToProto(result)
		if err != nil {
			return nil, fmt.Errorf("failed to convert read result to proto: %w", err)
		}
		materializations = append(materializations, protoResult)
	}

	// Create a new request with the updated materializations
	return &wasm.ResolveWithStickyRequest{
		ResolveRequest:   request.GetResolveRequest(),
		Materializations: materializations,
		NotProcessSticky: request.GetNotProcessSticky(),
	}, nil
}

func readResultToProto(result ReadResult) (*resolverinternal.ReadResult, error) {
	switch v := result.(type) {
	case *ReadResultVariant:
		var variant string
		if v.Variant() != nil {
			variant = *v.Variant()
		}
		return &resolverinternal.ReadResult{
			Result: &resolverinternal.ReadResult_VariantResult{
				VariantResult: &resolverinternal.VariantData{
					Materialization: v.Materialization(),
					Unit:            v.Unit(),
					Rule:            v.Rule(),
					Variant:         variant,
				},
			},
		}, nil
	case *ReadResultInclusion:
		return &resolverinternal.ReadResult{
			Result: &resolverinternal.ReadResult_InclusionResult{
				InclusionResult: &resolverinternal.InclusionData{
					Materialization: v.Materialization(),
					Unit:            v.Unit(),
					IsIncluded:      v.Included(),
				},
			},
		}, nil
	default:
		return nil, fmt.Errorf("unknown read result type: %T", result)
	}
}

func (m *materializationSupportedResolver) FlushAllLogs() (err error) {
	return m.current.FlushAllLogs()
}

func (m *materializationSupportedResolver) FlushAssignLogs() (err error) {
	return m.current.FlushAssignLogs()
}

func (m *materializationSupportedResolver) SetResolverState(request *wasm.SetResolverStateRequest) (err error) {
	return m.current.SetResolverState(request)
}

func (m *materializationSupportedResolver) Close(ctx context.Context) error {
	return m.current.Close(ctx)
}
