package confidence

import (
	"context"
	"fmt"

	lr "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/local_resolver"
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
		if len(success.GetUpdates()) > 0 {
			m.storeUpdates(success.GetUpdates())
		}
		return response, nil

	case *wasm.ResolveWithStickyResponse_MissingMaterializations_:
		missingMaterializations := result.MissingMaterializations
		// Try to load missing materializations from store
		updatedRequest, err := m.handleMissingMaterializations(request, missingMaterializations.GetItems())
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

func (m *materializationSupportedResolver) storeUpdates(updates []*wasm.ResolveWithStickyResponse_MaterializationUpdate) {
	// Convert protobuf updates to WriteOp slice
	writeOps := make([]WriteOp, len(updates))
	for i, update := range updates {
		writeOps[i] = newWriteOpVariant(
			update.GetWriteMaterialization(),
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
func (m *materializationSupportedResolver) handleMissingMaterializations(request *wasm.ResolveWithStickyRequest, missingItems []*wasm.ResolveWithStickyResponse_MissingMaterializationItem) (*wasm.ResolveWithStickyRequest, error) {
	// Convert missing items to ReadOp slice
	readOps := make([]ReadOp, len(missingItems))
	for i, item := range missingItems {
		readOps[i] = newReadOpVariant(
			item.GetReadMaterialization(),
			item.GetUnit(),
			item.GetRule(),
		)
	}

	// Read from the store
	results, err := m.store.Read(context.Background(), readOps)
	if err != nil {
		return nil, err
	}

	// Convert results to protobuf MaterializationMap format
	// Group by unit for efficiency
	materializationsPerUnit := make(map[string]*wasm.MaterializationMap)

	// Copy existing materializations
	for k, v := range request.GetMaterializationsPerUnit() {
		materializationsPerUnit[k] = v
	}

	// Add loaded materializations
	for _, result := range results {
		variantResult, ok := result.(*ReadResultVariant)
		if !ok {
			continue
		}

		unit := variantResult.Unit()
		mat := variantResult.Materialization()

		// Ensure the map exists for this unit
		if materializationsPerUnit[unit] == nil {
			materializationsPerUnit[unit] = &wasm.MaterializationMap{
				InfoMap: make(map[string]*wasm.MaterializationInfo),
			}
		}

		// Get or create the info for this materialization
		if materializationsPerUnit[unit].InfoMap[mat] == nil {
			materializationsPerUnit[unit].InfoMap[mat] = &wasm.MaterializationInfo{
				UnitInInfo:    false,
				RuleToVariant: make(map[string]string),
			}
		}

		// Add the variant if it exists
		if variantResult.Variant() != nil {
			materializationsPerUnit[unit].InfoMap[mat].RuleToVariant[variantResult.Rule()] = *variantResult.Variant()
			materializationsPerUnit[unit].InfoMap[mat].UnitInInfo = true
		}
	}

	// Create a new request with the updated materializations
	return &wasm.ResolveWithStickyRequest{
		ResolveRequest:          request.GetResolveRequest(),
		MaterializationsPerUnit: materializationsPerUnit,
		FailFastOnSticky:        request.GetFailFastOnSticky(),
		NotProcessSticky:        request.GetNotProcessSticky(),
	}, nil
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
