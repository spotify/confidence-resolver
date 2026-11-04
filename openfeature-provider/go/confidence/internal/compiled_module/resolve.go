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
// against it; Close frees the module-allocated block and returns the
// pinned instance to the pool.
type Envelope struct {
	host   *Host
	inst   *instance
	packed uint64
	closed bool
}

// Close frees the envelope's block in the module's shared memory and
// releases the pinned instance. Idempotent.
func (e *Envelope) Close() {
	if e == nil || e.closed {
		return
	}
	e.closed = true
	e.host.freePacked(e.inst, e.packed)
	e.host.release(e.inst)
	e.inst = nil
}

// Suspended is an in-flight resolve waiting on materializations only the
// host's storage layer can provide. The process handle is opaque and
// single-use; ResumeResolve or Discard consumes it. The process stays
// pinned to the instance that started it.
type Suspended struct {
	processID uint32
	inst      *instance
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

// StartResolve begins a resolve on a pool instance, which stays pinned to
// the returned flow. It returns either a completed result or a suspended
// process the caller resumes with ResumeResolve (or abandons with
// Discard).
func (h *Host) StartResolve(request *resolver.ResolveFlagsRequest) (*ResolveResult, *Suspended, error) {
	inst, err := h.acquire()
	if err != nil {
		return nil, nil, err
	}

	reqSlice, err := h.writeSlice(inst, mustMarshal(request))
	if err != nil {
		h.release(inst)
		return nil, nil, err
	}
	res, err := inst.resolveProcessStart.Call(context.Background(), reqSlice)
	if err != nil {
		h.release(inst)
		return nil, nil, err
	}
	resolved, suspended, envelope, err := h.decodeProcessResponse(inst, res[0])
	if err != nil {
		h.release(inst)
		return nil, nil, err
	}
	if suspended != nil {
		suspended.inst = inst
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
// host found in its store, on the instance that started it. Omitted records
// count as absent. A second suspension is a module defect and fails the
// resolve.
func (h *Host) ResumeResolve(s *Suspended, materializations *flagswasm.Materialization) (*ResolveResult, error) {
	if s == nil || s.processID == 0 || s.inst == nil {
		return nil, errors.New("no suspended process to resume")
	}
	id := s.processID
	inst := s.inst
	s.processID = 0
	s.inst = nil

	matSlice, err := h.writeSlice(inst, mustMarshal(materializations))
	if err != nil {
		h.discardLocked(inst, id)
		h.release(inst)
		return nil, err
	}
	res, err := inst.resolveProcessResume.Call(context.Background(), uint64(id), matSlice)
	if err != nil {
		// The handle may or may not have been consumed by the module;
		// discarding defensively releases any remaining process state.
		h.discardLocked(inst, id)
		h.release(inst)
		return nil, err
	}
	resolved, suspended, envelope, err := h.decodeProcessResponse(inst, res[0])
	if err != nil {
		h.discardLocked(inst, id)
		h.release(inst)
		return nil, err
	}
	if suspended != nil {
		// A second suspension is a module defect: discard the new handle
		// and fail the resolve.
		h.discardLocked(inst, suspended.processID)
		h.release(inst)
		return nil, errSecondSuspend
	}
	return &ResolveResult{
		Envelope:               envelope,
		Response:               resolved.GetResponse(),
		MaterializationToWrite: resolved.GetMaterializationToWrite(),
		ClientIndex:            resolved.GetClientIndex(),
	}, nil
}

// Discard abandons a suspended process, releasing its module-side state and
// the pinned instance.
func (h *Host) Discard(s *Suspended) {
	if s == nil || s.processID == 0 || s.inst == nil {
		return
	}
	id := s.processID
	inst := s.inst
	s.processID = 0
	s.inst = nil
	h.discardLocked(inst, id)
	h.release(inst)
}

// discardLocked calls resolve_process_discard on the process's pinned
// instance.
func (h *Host) discardLocked(inst *instance, processID uint32) {
	if processID == 0 {
		return
	}
	_, _ = inst.resolveProcessDiscard.Call(context.Background(), uint64(processID))
}

// Evaluate performs OpenFeature's evaluation step for one flag against a
// resolve envelope, on the instance the envelope is pinned to: splitting
// the key into flag and value path, walking the path, checking the
// resolved value against the caller's default, merging defaults into
// nulls and mapping errors. The envelope is borrowed; the caller frees it
// when done evaluating.
func (h *Host) Evaluate(envelope *Envelope, request *flagswasm.EvaluateRequest) (*flagswasm.ResolutionDetails, error) {
	if envelope == nil || envelope.closed || envelope.inst == nil {
		return nil, errNilEnvelope
	}
	inst := envelope.inst

	reqSlice, err := h.writeSlice(inst, mustMarshal(request))
	if err != nil {
		return nil, err
	}
	res, err := inst.evaluate.Call(context.Background(), envelope.packed, reqSlice)
	if err != nil {
		return nil, err
	}
	detailsBytes, err := h.readAndFreeSlice(inst, res[0])
	if err != nil {
		return nil, err
	}
	details := &flagswasm.ResolutionDetails{}
	if err := proto.Unmarshal(detailsBytes, details); err != nil {
		return nil, err
	}
	return details, nil
}

// ApplyFlags applies previously resolved flags (exposure logging for
// deferred applies). Any instance serves it: the apply state is module-wide
// in the shared memory. Non-zero statuses are best-effort logging
// failures: returned to the caller to log, never surfaced as evaluation
// errors.
func (h *Host) ApplyFlags(request *resolver.ApplyFlagsRequest) (ApplyStatus, error) {
	var status ApplyStatus
	err := h.withInstance(func(inst *instance) error {
		reqSlice, err := h.writeSlice(inst, mustMarshal(request))
		if err != nil {
			return err
		}
		res, err := inst.applyFlags.Call(context.Background(), reqSlice)
		if err != nil {
			return err
		}
		status = ApplyStatus(int32(res[0]))
		return nil
	})
	if err != nil {
		return ApplyInvalid, err
	}
	return status, nil
}

// FlushLogs asks the module to deliver buffered logs through write_logs.
// Any instance serves it: the log buffer is module-wide.
func (h *Host) FlushLogs() error {
	return h.withInstance(func(inst *instance) error {
		_, err := inst.flushLogs.Call(context.Background())
		return err
	})
}

// MetricsSnapshot returns module-wide metrics text (false = Prometheus,
// true = OpenMetrics). Empty when the module does not export
// metrics_snapshot.
func (h *Host) MetricsSnapshot(openmetrics bool) (string, error) {
	if !h.hasMetricsSnapshot {
		return "", nil
	}
	format := uint64(0)
	if openmetrics {
		format = 1
	}
	var text string
	err := h.withInstance(func(inst *instance) error {
		res, err := inst.metricsSnapshot.Call(context.Background(), format)
		if err != nil {
			return err
		}
		bytes, err := h.readAndFreeSlice(inst, res[0])
		if err != nil {
			return err
		}
		text = string(bytes)
		return nil
	})
	if err != nil {
		return "", err
	}
	return text, nil
}

// AllocatedBytes returns the module's allocation gauge and whether the
// module supports it.
func (h *Host) AllocatedBytes() (uint32, bool) {
	if !h.hasAllocatedBytes {
		return 0, false
	}
	var value uint32
	err := h.withInstance(func(inst *instance) error {
		res, err := inst.allocatedBytes.Call(context.Background())
		if err != nil {
			return err
		}
		value = uint32(res[0])
		return nil
	})
	if err != nil {
		return 0, true
	}
	return value, true
}

// ObserveResolveLatency reports one resolve's latency (from the start of the
// resolve through the final resume) to the module. No-op when the module
// does not export it or the client index is unknown. Any instance serves
// it: the metrics are module-wide.
func (h *Host) ObserveResolveLatency(clientIndex int32, micros int64) {
	if !h.hasObserveLatency || clientIndex <= 0 {
		return
	}
	_ = h.withInstance(func(inst *instance) error {
		_, err := inst.observeLatency.Call(context.Background(), uint64(uint32(clientIndex)), uint64(micros))
		return err
	})
}

// decodeProcessResponse decodes the ResolveProcessResponse slice returned by
// a start/resume call on the pinned instance. Exactly one of
// (resolved+envelope) or suspended is returned on success. The envelope
// block is freed on the suspended and error paths; on the resolved path
// ownership passes to the caller.
func (h *Host) decodeProcessResponse(inst *instance, packed uint64) (
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
		h.freePacked(inst, packed)
		return nil, nil, nil, err
	}
	switch result := process.Result.(type) {
	case *flagswasm.ResolveProcessResponse_Resolved_:
		return result.Resolved, nil, &Envelope{host: h, inst: inst, packed: packed}, nil
	case *flagswasm.ResolveProcessResponse_Suspended:
		// The suspended envelope is spent; the process handle carries the
		// state forward on the pinned instance.
		h.freePacked(inst, packed)
		return nil, &Suspended{
			processID: result.Suspended.GetProcessId(),
			toRead:    result.Suspended.GetMaterializationToRead(),
		}, nil, nil
	case *flagswasm.ResolveProcessResponse_Error:
		h.freePacked(inst, packed)
		return nil, nil, nil, &ResolveRejectedError{
			Code:    result.Error.GetCode(),
			Message: result.Error.GetMessage(),
		}
	default:
		h.freePacked(inst, packed)
		return nil, nil, nil, errEmptyEnvelope
	}
}

// freePacked frees a module-allocated block by packed slice on the instance
// that manages it.
func (h *Host) freePacked(inst *instance, packed uint64) {
	ptr, length := unpackSlice(packed)
	if length > 0 {
		_, _ = inst.frees.Call(context.Background(), uint64(ptr))
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
