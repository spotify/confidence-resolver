package confidence

import (
	"context"
	"fmt"

	lr "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/local_resolver"
	messages "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/proto/resolver"
)

// materializationSupportedResolver wraps a LocalResolver and allows it to read and write materializations.
type materializationSupportedResolver struct {
	store   MaterializationStore
	current lr.LocalResolver
}

func NewMaterializationSupportedResolver(store MaterializationStore, innerResolver lr.LocalResolver) *materializationSupportedResolver {
	return &materializationSupportedResolver{
		store:   store,
		current: innerResolver,
	}
}

func (m *materializationSupportedResolver) ResolveWithSticky(request *resolver.ResolveWithStickyRequest) (resp *resolver.ResolveWithStickyResponse, err error) {
	response, err := m.current.ResolveWithSticky(request)
	if err != nil {
		return nil, err
	}
	return m.handleStickyResponse(request, response)

}

func (m *materializationSupportedResolver) handleStickyResponse(request *resolver.ResolveWithStickyRequest, response *resolver.ResolveWithStickyResponse) (*resolver.ResolveWithStickyResponse, error) {
	switch result := response.ResolveResult.(type) {
	case *resolver.ResolveWithStickyResponse_Success_:
		success := result.Success
		// Store updates if present
		if len(success.GetUpdates()) > 0 {
			m.storeUpdates(success.GetUpdates())
		}
		return response, nil

	case *resolver.ResolveWithStickyResponse_MissingMaterializations_:
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
		return m.handleStickyResponse(updatedRequest, retryResponse)

	default:
		return nil, fmt.Errorf("unexpected resolve result type: %T", response.ResolveResult)

	}
}

func (m *materializationSupportedResolver) storeUpdates(updates []*resolver.ResolveWithStickyResponse_MaterializationUpdate) {
	// Convert protobuf updates to WriteOp slice
	writeOps := make([]WriteOp, len(updates))
	for i, update := range updates {
		writeOps[i] = NewWriteOpVariant(
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
func (m *materializationSupportedResolver) handleMissingMaterializations(request *resolver.ResolveWithStickyRequest, missingItems []*resolver.ResolveWithStickyResponse_MissingMaterializationItem) (*resolver.ResolveWithStickyRequest, error) {
	// Convert missing items to ReadOp slice
	readOps := make([]ReadOp, len(missingItems))
	for i, item := range missingItems {
		readOps[i] = NewReadOpVariant(
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
	materializationsPerUnit := make(map[string]*resolver.MaterializationMap)

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
			materializationsPerUnit[unit] = &resolver.MaterializationMap{
				InfoMap: make(map[string]*resolver.MaterializationInfo),
			}
		}

		// Get or create the info for this materialization
		if materializationsPerUnit[unit].InfoMap[mat] == nil {
			materializationsPerUnit[unit].InfoMap[mat] = &resolver.MaterializationInfo{
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
	return &resolver.ResolveWithStickyRequest{
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

func (m *materializationSupportedResolver) SetResolverState(request *messages.SetResolverStateRequest) (err error) {
	return m.current.SetResolverState(request)
}

func (m *materializationSupportedResolver) Close(ctx context.Context) error {
	return m.current.Close(ctx)
}

func wrapResolverSupplierWithMaterializations(supplier LocalResolverSupplier, materializationStore MaterializationStore) LocalResolverSupplier {
	return func(ctx context.Context, logSink lr.LogSink) lr.LocalResolver {
		localResolver := supplier(ctx, logSink)
		return NewMaterializationSupportedResolver(materializationStore, localResolver)
	}
}
