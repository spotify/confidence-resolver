package compiled_module

import (
	"context"
	"errors"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/flagswasm"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	"google.golang.org/protobuf/proto"
)

// Envelope holds the raw ResolveProcessResponse bytes a resolve produced.
// It is borrowed by Evaluate and must stay alive until the last evaluation
// against it; Close frees the module-allocated block.
type Envelope struct {
	host   *Host
	packed uint64
	closed bool
}

// Close frees the envelope's block in the module's shared memory. Idempotent.
func (e *Envelope) Close() {
	if e == nil || e.closed {
		return
	}
	e.closed = true
	e.host.mu.Lock()
	defer e.host.mu.Unlock()
	e.host.freePacked(e.packed)
}

// Suspended is an in-flight resolve waiting on materializations only the
// host's storage layer can provide. The process handle is opaque and
// single-use; ResumeResolve or Discard consumes it.
type Suspended struct {
	processID uint32
	toRead    []*flagswasm.Materialization_Record
}

// ToRead lists the materialization records the host must look up before
// resuming.
func (s *Suspended) ToRead() []*flagswasm.Materialization_Record {
	return s.toRead
}

// ResolveResult is a completed resolve. Envelope must be Closed by the caller
// after the last Evaluate against it.
type ResolveResult struct {
	Envelope               *Envelope
	Response               *resolver.ResolveFlagsResponse
	MaterializationToWrite []*flagswasm.Materialization_Record
	ClientIndex            int32
}

// StartResolve begins a resolve. It returns either a completed result or a
// suspended process the caller resumes with ResumeResolve (or abandons with
// Discard).
func (h *Host) StartResolve(request *resolver.ResolveFlagsRequest) (*ResolveResult, *Suspended, error) {
	h.mu.Lock()
	defer h.mu.Unlock()

	reqSlice, err := h.writeSlice(mustMarshal(request))
	if err != nil {
		return nil, nil, err
	}
	res, err := h.resolveProcessStart.Call(context.Background(), reqSlice)
	if err != nil {
		return nil, nil, err
	}
	resolved, suspended, envelope, err := h.decodeProcessResponse(res[0])
	if err != nil {
		return nil, nil, err
	}
	if suspended != nil {
		return nil, suspended, nil
	}
	return &ResolveResult{
		Envelope:               envelope,
		Response:               resolved.GetResponse(),
		MaterializationToWrite: resolved.GetMaterializationToWrite(),
		ClientIndex:            resolved.GetClientIndex(),
	}, nil, nil
}

// ResumeResolve resumes a suspended process with the materializations the
// host found in its store. Omitted records count as absent. A second
// suspension is a module defect and fails the resolve.
func (h *Host) ResumeResolve(s *Suspended, materializations *flagswasm.Materialization) (*ResolveResult, error) {
	if s == nil || s.processID == 0 {
		return nil, errors.New("no suspended process to resume")
	}
	id := s.processID

	h.mu.Lock()
	matSlice, err := h.writeSlice(mustMarshal(materializations))
	if err == nil {
		var res []uint64
		res, err = h.resolveProcessResume.Call(context.Background(), uint64(id), matSlice)
		if err == nil {
			var resolved *flagswasm.ResolveProcessResponse_Resolved
			var suspended *Suspended
			var envelope *Envelope
			resolved, suspended, envelope, err = h.decodeProcessResponse(res[0])
			if err == nil && suspended != nil {
				// A second suspension is a module defect: discard the new
				// handle and fail the resolve.
				h.discardLocked(suspended.processID)
				err = errSecondSuspend
			}
			if err == nil {
				s.processID = 0
				h.mu.Unlock()
				return &ResolveResult{
					Envelope:               envelope,
					Response:               resolved.GetResponse(),
					MaterializationToWrite: resolved.GetMaterializationToWrite(),
					ClientIndex:            resolved.GetClientIndex(),
				}, nil
			}
		}
	}
	h.mu.Unlock()

	// Error path: the handle may or may not have been consumed by the module;
	// discarding defensively releases any remaining process state.
	h.mu.Lock()
	h.discardLocked(id)
	h.mu.Unlock()
	s.processID = 0
	return nil, err
}

// Discard abandons a suspended process, releasing its module-side state.
func (h *Host) Discard(s *Suspended) {
	if s == nil || s.processID == 0 {
		return
	}
	id := s.processID
	s.processID = 0
	h.mu.Lock()
	defer h.mu.Unlock()
	h.discardLocked(id)
}

// discardLocked calls resolve_process_discard. Entry mutex must be held.
func (h *Host) discardLocked(processID uint32) {
	if processID == 0 {
		return
	}
	_, _ = h.resolveProcessDiscard.Call(context.Background(), uint64(processID))
}

// Evaluate performs OpenFeature's evaluation step for one flag against a
// resolve envelope: splitting the key into flag and value path, walking the
// path, checking the resolved value against the caller's default, merging
// defaults into nulls and mapping errors. The envelope is borrowed; the
// caller frees it when done evaluating.
func (h *Host) Evaluate(envelope *Envelope, request *flagswasm.EvaluateRequest) (*flagswasm.ResolutionDetails, error) {
	if envelope == nil || envelope.closed {
		return nil, errNilEnvelope
	}
	h.mu.Lock()
	defer h.mu.Unlock()

	reqSlice, err := h.writeSlice(mustMarshal(request))
	if err != nil {
		return nil, err
	}
	res, err := h.evaluate.Call(context.Background(), envelope.packed, reqSlice)
	if err != nil {
		return nil, err
	}
	detailsBytes, err := h.readAndFreeSlice(res[0])
	if err != nil {
		return nil, err
	}
	details := &flagswasm.ResolutionDetails{}
	if err := proto.Unmarshal(detailsBytes, details); err != nil {
		return nil, err
	}
	return details, nil
}

// ApplyFlags applies previously resolved flags (exposure logging for deferred
// applies). Non-zero statuses are best-effort logging failures: returned to
// the caller to log, never surfaced as evaluation errors.
func (h *Host) ApplyFlags(request *resolver.ApplyFlagsRequest) (ApplyStatus, error) {
	h.mu.Lock()
	defer h.mu.Unlock()
	reqSlice, err := h.writeSlice(mustMarshal(request))
	if err != nil {
		return ApplyInvalid, err
	}
	res, err := h.applyFlags.Call(context.Background(), reqSlice)
	if err != nil {
		return ApplyInvalid, err
	}
	return ApplyStatus(int32(res[0])), nil
}

// FlushLogs asks the module to deliver buffered logs through write_logs.
func (h *Host) FlushLogs() error {
	h.mu.Lock()
	defer h.mu.Unlock()
	_, err := h.flushLogs.Call(context.Background())
	return err
}

// MetricsSnapshot returns module-wide metrics text (false = Prometheus, true
// = OpenMetrics). Empty when the module does not export metrics_snapshot.
func (h *Host) MetricsSnapshot(openmetrics bool) (string, error) {
	if !h.hasMetricsSnapshot {
		return "", nil
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	format := int32(0)
	if openmetrics {
		format = 1
	}
	res, err := h.metricsSnapshot.Call(context.Background(), uint64(format))
	if err != nil {
		return "", err
	}
	text, err := h.readAndFreeSlice(res[0])
	if err != nil {
		return "", err
	}
	return string(text), nil
}

// AllocatedBytes returns the module's allocation gauge and whether the module
// supports it.
func (h *Host) AllocatedBytes() (uint32, bool) {
	if !h.hasAllocatedBytes {
		return 0, false
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	res, err := h.allocatedBytes.Call(context.Background())
	if err != nil {
		return 0, true
	}
	return uint32(res[0]), true
}

// ObserveResolveLatency reports one resolve's latency (from the start of the
// resolve through the final resume) to the module. No-op when the module
// does not export it or the client index is unknown.
func (h *Host) ObserveResolveLatency(clientIndex int32, micros int64) {
	if !h.hasObserveLatency || clientIndex <= 0 {
		return
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	_, _ = h.observeLatency.Call(context.Background(), uint64(uint32(clientIndex)), uint64(micros))
}

// decodeProcessResponse decodes the ResolveProcessResponse slice returned by
// a start/resume call. Exactly one of (resolved+envelope) or suspended is
// returned on success. The envelope block is freed on the suspended and error
// paths; on the resolved path ownership passes to the caller.
// Entry mutex must be held.
func (h *Host) decodeProcessResponse(packed uint64) (
	*flagswasm.ResolveProcessResponse_Resolved,
	*Suspended,
	*Envelope,
	error,
) {
	if packed == 0 {
		return nil, nil, nil, errEmptyEnvelope
	}
	raw, err := h.readSlice(packed)
	if err != nil {
		return nil, nil, nil, err
	}
	process := &flagswasm.ResolveProcessResponse{}
	if err := proto.Unmarshal(raw, process); err != nil {
		h.freePacked(packed)
		return nil, nil, nil, err
	}
	switch result := process.Result.(type) {
	case *flagswasm.ResolveProcessResponse_Resolved_:
		return result.Resolved, nil, &Envelope{host: h, packed: packed}, nil
	case *flagswasm.ResolveProcessResponse_Suspended:
		// The suspended envelope is spent; the process handle carries the
		// state forward.
		h.freePacked(packed)
		return nil, &Suspended{
			processID: result.Suspended.GetProcessId(),
			toRead:    result.Suspended.GetMaterializationToRead(),
		}, nil, nil
	case *flagswasm.ResolveProcessResponse_Error:
		h.freePacked(packed)
		return nil, nil, nil, &ResolveRejectedError{
			Code:    result.Error.GetCode(),
			Message: result.Error.GetMessage(),
		}
	default:
		h.freePacked(packed)
		return nil, nil, nil, errEmptyEnvelope
	}
}

// freePacked frees a module-allocated block by packed slice. Entry mutex must
// be held.
func (h *Host) freePacked(packed uint64) {
	ptr, length := unpackSlice(packed)
	if length > 0 {
		_, _ = h.frees.Call(context.Background(), uint64(ptr))
	}
}

func mustMarshal(message proto.Message) []byte {
	data, err := proto.Marshal(message)
	if err != nil {
		// These messages have no required fields; marshalling cannot fail.
		panic(err)
	}
	return data
}
