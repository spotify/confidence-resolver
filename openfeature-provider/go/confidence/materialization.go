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
	// Convert to a simple Resolve so the inner resolver can suspend when
	// materializations are needed — the wrapper handles the suspend/resume cycle.
	innerRequest := request
	if rwm, ok := request.Request.(*wasm.ResolveProcessRequest_ResolveWithMaterializations_); ok {
		innerRequest = &wasm.ResolveProcessRequest{
			Request: &wasm.ResolveProcessRequest_Resolve{
				Resolve: rwm.ResolveWithMaterializations.ResolveRequest,
			},
		}
	}
	response, err := m.current.ResolveProcess(innerRequest)
	if err != nil {
		return nil, err
	}
	return m.handleResponseWithDepth(innerRequest, response, 0)
}

// max number of recursive retries when handling missing materializations
const maxStickyRetryDepth = 5

func (m *materializationSupportedResolver) handleResponseWithDepth(request *wasm.ResolveProcessRequest, response *wasm.ResolveProcessResponse, depth int) (*wasm.ResolveProcessResponse, error) {
	if depth >= maxStickyRetryDepth {
		return nil, fmt.Errorf("exceeded maximum retries (%d) for handling missing materializations", maxStickyRetryDepth)
	}

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
			Request: &wasm.ResolveProcessRequest_Resume{
				Resume: &wasm.ResolveProcessRequest_ResumeRequest{
					Materializations: materializations,
					State:            suspended.GetState(),
				},
			},
		}

		retryResponse, err := m.current.ResolveProcess(resumeRequest)
		if err != nil {
			return nil, err
		}
		return m.handleResponseWithDepth(resumeRequest, retryResponse, depth+1)

	default:
		return nil, fmt.Errorf("unexpected resolve result type: %T", response.Result)
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
			variant := ""
			if v.Variant() != nil {
				variant = *v.Variant()
			}
			records = append(records, &wasm.MaterializationRecord{
				Unit:            v.Unit(),
				Materialization: v.Materialization(),
				Rule:            v.Rule(),
				Variant:         variant,
			})
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
