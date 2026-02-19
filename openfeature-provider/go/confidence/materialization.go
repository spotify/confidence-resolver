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

func (m *materializationSupportedResolver) ResolveProcess(request *wasm.ResolveProcessRequest) (resp *wasm.ResolveProcessResponse, err error) {
	// Convert WithoutMaterializations to DeferredMaterializations so the inner resolver
	// can suspend when materializations are needed — the wrapper handles the suspend/resume cycle.
	innerRequest := request
	if req, ok := request.Resolve.(*wasm.ResolveProcessRequest_WithoutMaterializations); ok {
		innerRequest = &wasm.ResolveProcessRequest{
			Resolve: &wasm.ResolveProcessRequest_DeferredMaterializations{
				DeferredMaterializations: req.WithoutMaterializations,
			},
		}
	}
	response, err := m.current.ResolveProcess(innerRequest)
	if err != nil {
		return nil, err
	}
	return m.handleResponse(response)
}

func (m *materializationSupportedResolver) handleResponse(response *wasm.ResolveProcessResponse) (*wasm.ResolveProcessResponse, error) {
	switch result := response.Result.(type) {
	case *wasm.ResolveProcessResponse_Resolved_:
		resolved := result.Resolved
		// Store writes if present
		if len(resolved.GetMaterializationsToWrite()) > 0 {
			m.storeWrites(resolved.GetMaterializationsToWrite())
		}
		return response, nil

	case *wasm.ResolveProcessResponse_Suspended_:
		suspended := result.Suspended
		// Convert MaterializationRecord to ReadOps and fetch from store
		readOps := materializationRecordsToReadOps(suspended.GetMaterializationsToRead())
		results, err := m.store.Read(context.Background(), readOps)
		if err != nil {
			return nil, fmt.Errorf("failed to read materializations: %w", err)
		}

		// Convert read results back to MaterializationRecords for the resume
		materializations := readResultsToMaterializationRecords(results)

		// Resume with the fetched materializations
		resumeRequest := &wasm.ResolveProcessRequest{
			Resolve: &wasm.ResolveProcessRequest_Resume_{
				Resume: &wasm.ResolveProcessRequest_Resume{
					Materializations: materializations,
					State:            suspended.GetState(),
				},
			},
		}

		resumeResponse, err := m.current.ResolveProcess(resumeRequest)
		if err != nil {
			return nil, err
		}
		return m.handleResumeResponse(resumeResponse)

	default:
		return nil, fmt.Errorf("unexpected resolve result type: %T", response.Result)
	}
}

func (m *materializationSupportedResolver) handleResumeResponse(response *wasm.ResolveProcessResponse) (*wasm.ResolveProcessResponse, error) {
	switch result := response.Result.(type) {
	case *wasm.ResolveProcessResponse_Resolved_:
		resolved := result.Resolved
		// Store writes if present
		if len(resolved.GetMaterializationsToWrite()) > 0 {
			m.storeWrites(resolved.GetMaterializationsToWrite())
		}
		return response, nil

	case *wasm.ResolveProcessResponse_Suspended_:
		return nil, fmt.Errorf("unexpected second suspend after resume")

	default:
		return nil, fmt.Errorf("unexpected resolve result type after resume: %T", response.Result)
	}
}

func (m *materializationSupportedResolver) storeWrites(records []*wasm.MaterializationRecord) {
	writeOps := make([]WriteOp, len(records))
	for i, record := range records {
		writeOps[i] = newWriteOpVariant(
			record.GetMaterialization(),
			record.GetUnit(),
			record.GetRule(),
			record.GetVariant(),
		)
	}

	go func() {
		if err := m.store.Write(context.Background(), writeOps); err != nil {
			if _, ok := err.(*MaterializationNotSupportedError); !ok {
				// TODO: Add proper logging
				_ = err
			}
		}
	}()
}

// materializationRecordsToReadOps converts MaterializationRecord protos to ReadOp interface values.
// Records with a non-empty rule are treated as variant reads; others as inclusion reads.
func materializationRecordsToReadOps(records []*wasm.MaterializationRecord) []ReadOp {
	ops := make([]ReadOp, len(records))
	for i, record := range records {
		if record.GetRule() != "" {
			ops[i] = newReadOpVariant(record.GetMaterialization(), record.GetUnit(), record.GetRule())
		} else {
			ops[i] = newReadOpInclusion(record.GetMaterialization(), record.GetUnit())
		}
	}
	return ops
}

// readResultsToMaterializationRecords converts ReadResult interface values back to
// MaterializationRecord protos for a Resume request. Inclusion results that are not
// included are omitted (absence = not included).
func readResultsToMaterializationRecords(results []ReadResult) []*wasm.MaterializationRecord {
	var records []*wasm.MaterializationRecord
	for _, result := range results {
		switch v := result.(type) {
		case *ReadResultVariant:
			if v.Variant() != nil && *v.Variant() != "" {
				records = append(records, &wasm.MaterializationRecord{
					Unit:            v.Unit(),
					Materialization: v.Materialization(),
					Rule:            v.Rule(),
					Variant:         *v.Variant(),
				})
			}
			// No prior assignment → omit (absence = no sticky assignment)
		case *ReadResultInclusion:
			if v.Included() {
				records = append(records, &wasm.MaterializationRecord{
					Unit:            v.Unit(),
					Materialization: v.Materialization(),
				})
			}
			// Not included → omit (absence = not included)
		}
	}
	return records
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
