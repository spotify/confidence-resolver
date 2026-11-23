package host

import (
	"context"
	"crypto/rand"
	"encoding/binary"
	"errors"
	"fmt"
	"sync"
	"sync/atomic"
	"time"

	"github.com/tetratelabs/wazero"
	"github.com/tetratelabs/wazero/api"
	"google.golang.org/protobuf/proto"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/s2wpb/resolverpb"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/s2wpb/wasmpb"
)

// ErrClosed is returned by calls on a closed Resolver.
var ErrClosed = errors.New("resolver is closed")

// ErrPoisoned is returned by calls on a Resolver after a module call failed (see Config.OnPoison).
var ErrPoisoned = errors.New("resolver module trapped; its state may be inconsistent")

// MaterializationStore is the storage boundary for suspended resolves.
type MaterializationStore interface {
	// Read answers the requested records. Omitted records are absent.
	Read(ctx context.Context, requested []*wasmpb.Materialization_Record) ([]*wasmpb.Materialization_Record, error)
	Write(ctx context.Context, assignments []*wasmpb.Materialization_Record) error
}

// Config configures a Resolver. Zero values select defaults.
type Config struct {
	// Instances bounds concurrent calls into the module. Default 1.
	Instances int
	// MaxMemoryBytes caps the shared memory. Default 64 MiB, and never above the module's declared max.
	MaxMemoryBytes uint64
	// WriteLogs receives each serialized WriteFlagLogsRequest. It owns the slice. It must not block
	// or call back into the Resolver.
	WriteLogs func([]byte)
	// ApplyFilter and SchemaFilter answer the module's dedup imports. Default: NeverSeen.
	ApplyFilter, SchemaFilter Filter
	// ReportFailure receives errors raised inside host callbacks.
	ReportFailure func(error)
	// OnPoison is called once, with the error, when a module call fails (a trap). The module's
	// shared state may then be inconsistent: later calls fail with ErrPoisoned, calls in flight
	// on other instances may never return, and Close abandons the module without calling into it.
	// It must not block.
	OnPoison func(error)
}

// DefaultMaxMemoryBytes is the default shared-memory cap.
const DefaultMaxMemoryBytes = 64 << 20

// Resolver hosts one module generation: its own wazero runtime, shared memory and instance pool.
type Resolver struct {
	rt      *Runtime
	module  *Module
	memory  api.Module
	filters map[string]linkedFilter // by import module name
	cfg     Config
	free    chan *instance
	mu      sync.Mutex // guards created and all
	created int
	all     []*instance
	closed  atomic.Bool
	poison  atomic.Bool
	// raceToken orders Go-side shared-memory accesses for the race detector (see race.go).
	raceToken byte
}

type instance struct {
	mod                                                     api.Module
	alloc, free, start, resume, discard, apply, flush, eval api.Function
	destroy                                                 api.Function
	stack                                                   [2]uint64
	race                                                    *byte        // see race.go
	latency                                                 api.Function // observe_client_resolve_latency, if exported
	discards                                                int          // resolve_process_discard calls, for tests
	poison                                                  func(error)
}

// callCtx is passed to every call into an instance. Callers' contexts are deliberately not used:
// without WithCloseOnContextDone (which would kill the module), wazero only hands the context to
// host functions, and none of ours need it.
var callCtx = context.Background()

// NewResolver compiles wasm in a fresh runtime and prepares its shared memory. Instances are
// created lazily.
func NewResolver(ctx context.Context, wasm []byte, interpreter bool, cfg Config) (*Resolver, error) {
	if cfg.Instances <= 0 {
		cfg.Instances = 1
	}
	if cfg.MaxMemoryBytes == 0 {
		cfg.MaxMemoryBytes = DefaultMaxMemoryBytes
	}
	if cfg.ApplyFilter == nil {
		cfg.ApplyFilter = NeverSeen
	}
	if cfg.SchemaFilter == nil {
		cfg.SchemaFilter = NeverSeen
	}
	if cfg.WriteLogs == nil {
		cfg.WriteLogs = func([]byte) {}
	}
	if cfg.ReportFailure == nil {
		cfg.ReportFailure = func(error) {}
	}
	if cfg.OnPoison == nil {
		cfg.OnPoison = func(error) {}
	}
	rt := NewRuntime(ctx, interpreter)
	r := &Resolver{rt: rt, cfg: cfg, free: make(chan *instance, cfg.Instances)}
	if err := r.init(ctx, wasm); err != nil {
		_ = rt.Close(ctx)
		return nil, err
	}
	return r, nil
}

func (r *Resolver) init(ctx context.Context, wasm []byte) error {
	m, err := r.rt.Compile(ctx, wasm)
	if err != nil {
		return err
	}
	r.module = m
	maxPages := min(m.MaxPages, uint32(r.cfg.MaxMemoryBytes/pageSize))
	if maxPages < m.MinPages {
		return fmt.Errorf("memory cap of %d bytes is below the module's minimum of %d pages", r.cfg.MaxMemoryBytes, m.MinPages)
	}
	r.memory, err = r.rt.r.InstantiateWithConfig(ctx, sharedMemoryModule(m.MinPages, maxPages), wazero.NewModuleConfig().WithName("host"))
	if err != nil {
		return fmt.Errorf("shared memory: %w", err)
	}
	_, err = r.rt.r.NewHostModuleBuilder("env").
		NewFunctionBuilder().WithFunc(r.writeLogs).Export("write_logs").
		NewFunctionBuilder().WithFunc(func() int64 { return time.Now().UnixMilli() }).Export("current_time").
		NewFunctionBuilder().WithFunc(randomSeed).Export("random_seed").
		Instantiate(ctx)
	if err != nil {
		return err
	}
	r.filters = map[string]linkedFilter{}
	for name, f := range map[string]Filter{"apply_filter": r.cfg.ApplyFilter, "schema_filter": r.cfg.SchemaFilter} {
		if r.filters[name], err = linkFilter(ctx, r.rt.r, f); err != nil {
			return fmt.Errorf("%s: %w", name, err)
		}
	}
	return nil
}

func randomSeed() int64 {
	var b [8]byte
	_, _ = rand.Read(b[:])
	return int64(binary.LittleEndian.Uint64(b[:]))
}

// claim returns an idle instance, creating one while under the cap. It blocks while all are busy.
func (r *Resolver) claim(ctx context.Context) (*instance, error) {
	select {
	case inst := <-r.free:
		return inst, nil
	default:
	}
	r.mu.Lock()
	if r.closed.Load() {
		r.mu.Unlock()
		return nil, ErrClosed
	}
	if r.created < r.cfg.Instances {
		r.created++
		r.mu.Unlock()
		inst, err := r.newInstance(ctx)
		if err != nil {
			r.mu.Lock()
			r.created--
			r.mu.Unlock()
			return nil, err
		}
		r.mu.Lock()
		r.all = append(r.all, inst)
		r.mu.Unlock()
		return inst, nil
	}
	r.mu.Unlock()
	select {
	case inst := <-r.free:
		return inst, nil
	case <-ctx.Done():
		return nil, ctx.Err()
	}
}

func (r *Resolver) release(inst *instance) { r.free <- inst }

func (r *Resolver) newInstance(ctx context.Context) (*instance, error) {
	// Each instance links to filter instances of its own. On error, what was instantiated is
	// released with the runtime.
	links := make(map[string]api.Module, len(r.filters))
	for name, f := range r.filters {
		m, err := f.instantiate(ctx, r.rt.r)
		if err != nil {
			return nil, fmt.Errorf("%s: %w", name, err)
		}
		links[name] = m
	}
	mod, err := r.rt.r.InstantiateModule(importFrom(ctx, links), r.module.compiled, wazero.NewModuleConfig().WithName(""))
	if err != nil {
		return nil, fmt.Errorf("instantiate: %w", err)
	}
	e := mod.ExportedFunction
	inst := &instance{
		race: &r.raceToken, poison: r.poisoned, mod: mod, alloc: e("alloc"), free: e("free"), start: e("resolve_process_start"),
		resume: e("resolve_process_resume"), discard: e("resolve_process_discard"), apply: e("apply_flags"),
		flush: e("flush_logs"), eval: e("evaluate"), destroy: e("destroy_instance"),
	}
	if r.module.Optional["observe_client_resolve_latency"] {
		inst.latency = e("observe_client_resolve_latency")
	}
	return inst, nil
}

// poisoned marks r poisoned by err, a failed module call. Only the first failure is reported.
func (r *Resolver) poisoned(err error) {
	if !r.poison.Swap(true) {
		r.cfg.OnPoison(err)
	}
}

// Poisoned reports whether a module call has failed (see Config.OnPoison).
func (r *Resolver) Poisoned() bool { return r.poison.Load() }

// with runs f on a claimed instance. ctx only bounds the wait for an instance; f receives the
// instance's own call context.
func (r *Resolver) with(ctx context.Context, f func(ctx context.Context, inst *instance) error) error {
	if r.closed.Load() {
		return ErrClosed
	}
	if r.poison.Load() {
		return ErrPoisoned
	}
	inst, err := r.claim(ctx)
	if err != nil {
		return err
	}
	defer r.release(inst)
	return f(callCtx, inst)
}

// Evaluate resolves req and evaluates each of evals against the result. It suspends to store
// when the module needs materializations.
func (r *Resolver) Evaluate(ctx context.Context, req *resolverpb.ResolveFlagsRequest, evals []*wasmpb.EvaluateRequest, store MaterializationStore) ([]*wasmpb.ResolutionDetails, error) {
	out, err := r.process(ctx, req, evals, false, store)
	return out.details, err
}

// Resolve resolves req and returns the module's full response (for deferred apply, which needs its
// resolve token).
func (r *Resolver) Resolve(ctx context.Context, req *resolverpb.ResolveFlagsRequest, store MaterializationStore) (*resolverpb.ResolveFlagsResponse, error) {
	out, err := r.process(ctx, req, nil, true, store)
	return out.response, err
}

// outcome is what one start or resume call produced.
type outcome struct {
	details   []*wasmpb.ResolutionDetails
	response  *resolverpb.ResolveFlagsResponse // only when requested
	writes    []*wasmpb.Materialization_Record
	suspended *suspension
}

func (r *Resolver) process(ctx context.Context, req *resolverpb.ResolveFlagsRequest, evals []*wasmpb.EvaluateRequest, wantResponse bool, store MaterializationStore) (outcome, error) {
	started := time.Now()
	reqBytes, err := proto.Marshal(req)
	if err != nil {
		return outcome{}, err
	}
	var out outcome
	err = r.with(ctx, func(ctx context.Context, inst *instance) error {
		arg, err := inst.write(ctx, reqBytes)
		if err != nil {
			return err
		}
		env, err := inst.call1(ctx, inst.start, arg)
		if err != nil {
			return err
		}
		out, err = r.finish(ctx, inst, env, evals, wantResponse, started)
		return err
	})
	if err != nil || out.suspended == nil {
		return out, r.write(ctx, store, out.writes, err)
	}

	// From here this call owns the suspended process: every path that doesn't resume it discards
	// it, or its context stays allocated in the module's memory.
	id := out.suspended.processID
	records, err := store.Read(ctx, out.suspended.reads)
	var mat []byte
	if err != nil {
		err = fmt.Errorf("materialization read: %w", err)
	} else {
		mat, err = proto.Marshal(&wasmpb.Materialization{Records: records})
	}
	if err != nil {
		return outcome{}, errors.Join(err, r.discard(ctx, id))
	}
	resumed := false
	err = r.with(ctx, func(ctx context.Context, inst *instance) error {
		arg, err := inst.write(ctx, mat)
		if err != nil {
			return err // not resumed: discarded below, once
		}
		resumed = true
		env, err := inst.call1(ctx, inst.resume, uint64(id), arg)
		if err != nil {
			return err
		}
		out, err = r.finish(ctx, inst, env, evals, wantResponse, started)
		if err == nil && out.suspended != nil {
			err = errors.Join(errors.New("resolve suspended after resume"), inst.discardProcess(ctx, out.suspended.processID))
		}
		return err
	})
	if err != nil && !resumed {
		// No instance (ctx ended while the pool was busy): the process is still ours to discard.
		err = errors.Join(err, r.discard(ctx, id))
	}
	if err != nil {
		return outcome{}, err
	}
	return out, r.write(ctx, store, out.writes, nil)
}

// discard discards a suspended process on any instance. It waits for one even if ctx has ended,
// which is bounded: instances are held only for module calls. A closed resolver has freed it.
func (r *Resolver) discard(ctx context.Context, id uint32) error {
	err := r.with(context.WithoutCancel(ctx), func(ctx context.Context, inst *instance) error {
		return inst.discardProcess(ctx, id)
	})
	if errors.Is(err, ErrClosed) {
		return nil
	}
	return err
}

func (i *instance) discardProcess(ctx context.Context, id uint32) error {
	i.discards++
	_, err := i.call(ctx, i.discard, uint64(id))
	return err
}

func (r *Resolver) write(ctx context.Context, store MaterializationStore, writes []*wasmpb.Materialization_Record, err error) error {
	if err != nil || len(writes) == 0 {
		return err
	}
	if werr := store.Write(ctx, writes); werr != nil {
		return fmt.Errorf("materialization write: %w", werr)
	}
	return nil
}

// finish decodes a process envelope at env and frees it. If the resolve completed, it reports the
// latency since started to the module and evaluates against the envelope.
func (r *Resolver) finish(ctx context.Context, inst *instance, env uint64, evals []*wasmpb.EvaluateRequest, wantResponse bool, started time.Time) (outcome, error) {
	defer inst.freeSlice(ctx, env)
	raceAcquire(inst.race)
	b, ok := inst.mod.Memory().Read(uint32(env), uint32(env>>32))
	e, err := decodeEnvelope(b)
	var response *resolverpb.ResolveFlagsResponse
	if err == nil && wantResponse && e.resolved {
		response = &resolverpb.ResolveFlagsResponse{}
		err = proto.Unmarshal(e.response, response)
	}
	raceRelease(inst.race)
	switch {
	case !ok:
		return outcome{}, errors.New("slice out of bounds")
	case err != nil:
		return outcome{}, err
	case e.err != nil:
		return outcome{}, e.err
	case e.suspended != nil:
		return outcome{suspended: e.suspended}, nil
	}
	if inst.latency != nil {
		_, _ = inst.call(ctx, inst.latency, uint64(e.clientIndex), uint64(time.Since(started).Microseconds()))
	}
	details := make([]*wasmpb.ResolutionDetails, len(evals))
	for i, ev := range evals {
		req, err := proto.Marshal(ev)
		if err != nil {
			return outcome{}, err
		}
		arg, err := inst.write(ctx, req)
		if err != nil {
			return outcome{}, err
		}
		out, err := inst.call1(ctx, inst.eval, env, arg)
		if err != nil {
			return outcome{}, err
		}
		details[i] = &wasmpb.ResolutionDetails{}
		err = inst.unmarshal(out, details[i])
		inst.freeSlice(ctx, out)
		if err != nil {
			return outcome{}, err
		}
	}
	return outcome{details: details, response: response, writes: e.writes}, nil
}

// Metrics renders the module's cumulative telemetry: Prometheus text, or OpenMetrics. It returns ""
// for modules without metrics_snapshot.
func (r *Resolver) Metrics(ctx context.Context, openMetrics bool) (string, error) {
	if !r.module.Optional["metrics_snapshot"] {
		return "", nil
	}
	format := uint64(0)
	if openMetrics {
		format = 1
	}
	var text string
	err := r.with(ctx, func(ctx context.Context, inst *instance) error {
		slice, err := inst.call1(ctx, inst.mod.ExportedFunction("metrics_snapshot"), format)
		if err != nil {
			return err
		}
		defer inst.freeSlice(ctx, slice)
		raceAcquire(inst.race)
		b, ok := inst.mod.Memory().Read(uint32(slice), uint32(slice>>32))
		text = string(b)
		raceRelease(inst.race)
		if !ok {
			return errors.New("slice out of bounds")
		}
		return nil
	})
	return text, err
}

// ResolveError is a resolve the module rejected.
type ResolveError struct {
	Code    wasmpb.ResolveProcessResponse_Error_Code
	Message string
}

func (e *ResolveError) Error() string {
	return fmt.Sprintf("resolve rejected (%v): %s", e.Code, e.Message)
}

// applyUnknownClient is apply_flags' status for a client secret that matches no client.
const applyUnknownClient = 4

// ApplyFlags forwards a serialized ApplyFlagsRequest.
func (r *Resolver) ApplyFlags(ctx context.Context, req []byte) error {
	return r.with(ctx, func(ctx context.Context, inst *instance) error {
		arg, err := inst.write(ctx, req)
		if err != nil {
			return err
		}
		res, err := inst.call(ctx, inst.apply, arg)
		if err != nil {
			return err
		}
		switch status := int32(res); status {
		case 0:
			return nil
		case applyUnknownClient:
			return &ResolveError{Code: wasmpb.ResolveProcessResponse_Error_UNAUTHENTICATED, Message: "client secret matches no client"}
		default:
			return fmt.Errorf("apply_flags failed with status %d", status)
		}
	})
}

// FlushLogs asks the module to emit buffered logs through Config.WriteLogs.
func (r *Resolver) FlushLogs(ctx context.Context) error {
	return r.with(ctx, func(ctx context.Context, inst *instance) error {
		_, err := inst.call(ctx, inst.flush)
		return err
	})
}

// Warm loads every segment bitset up front, if the module supports it, so the first resolves
// don't pay for inflating them. Bitsets live in shared memory, so one instance warms them all.
func (r *Resolver) Warm(ctx context.Context) error {
	if !r.module.Optional["unpack_bitsets"] {
		return nil
	}
	return r.with(ctx, func(ctx context.Context, inst *instance) error {
		_, err := inst.call(ctx, inst.mod.ExportedFunction("unpack_bitsets"))
		return err
	})
}

// Close flushes logs, retires every instance and releases the runtime. The caller must ensure no
// calls are in flight (internal/engine retires a generation only after draining it); later calls
// fail with ErrClosed.
func (r *Resolver) Close(ctx context.Context) error {
	if r.closed.Swap(true) {
		return nil
	}
	if r.poison.Load() {
		// Abandoned: a call into the module could block on a lock the failed call holds, and the
		// runtime can't be closed under calls that may still be running.
		return nil
	}
	var errs []error
	for i, inst := range r.all {
		if i == 0 {
			if _, err := inst.flush.Call(callCtx); err != nil {
				errs = append(errs, fmt.Errorf("final flush: %w", err))
			}
		}
		if _, err := inst.destroy.Call(callCtx); err != nil {
			errs = append(errs, err)
		}
	}
	errs = append(errs, r.rt.Close(ctx))
	return errors.Join(errs...)
}

// writeLogs is the env.write_logs import: copy the payload's parts into one owned buffer, hand
// it off, then ack on the calling instance.
func (r *Resolver) writeLogs(ctx context.Context, m api.Module, payload uint64) {
	defer func() {
		if p := recover(); p != nil {
			r.cfg.ReportFailure(fmt.Errorf("write_logs panicked: %v", p))
		}
		if _, err := m.ExportedFunction("ack_logs").Call(ctx, payload); err != nil {
			r.poisoned(fmt.Errorf("ack_logs: %w", err))
		}
	}()
	raceAcquire(&r.raceToken)
	defer raceRelease(&r.raceToken)
	mem := m.Memory()
	envBytes, ok := mem.Read(uint32(payload), uint32(payload>>32))
	if !ok {
		r.cfg.ReportFailure(errors.New("write_logs payload out of bounds"))
		return
	}
	var env wasmpb.WriteLogsPayload
	if err := proto.Unmarshal(envBytes, &env); err != nil {
		r.cfg.ReportFailure(fmt.Errorf("write_logs payload: %w", err))
		return
	}
	size := 0
	for _, p := range env.GetParts() {
		size += int(uint32(p >> 32))
	}
	buf := make([]byte, 0, size)
	for _, p := range env.GetParts() {
		part, ok := mem.Read(uint32(p), uint32(p>>32))
		if !ok {
			r.cfg.ReportFailure(errors.New("write_logs part out of bounds"))
			return
		}
		buf = append(buf, part...)
	}
	r.cfg.WriteLogs(buf)
}

// ErrAlloc is a module allocation that failed: its memory is exhausted.
var ErrAlloc = errors.New("module allocation failed")

// write copies b into a fresh module allocation and returns it as a slice argument. A failed
// allocation is an error: a null slice means something else to some exports (to resume, no
// materialization support).
func (i *instance) write(ctx context.Context, b []byte) (uint64, error) {
	res, err := i.call(ctx, i.alloc, uint64(len(b)))
	if err != nil {
		return 0, err
	}
	if uint32(res) == 0 && len(b) > 0 {
		return 0, fmt.Errorf("%w (%d bytes)", ErrAlloc, len(b))
	}
	ptr := uint32(res)
	raceAcquire(i.race)
	i.mod.Memory().Write(ptr, b)
	raceRelease(i.race)
	return uint64(ptr) | uint64(len(b))<<32, nil
}

// call calls f with args through the instance's reusable stack, avoiding the results slice that
// api.Function.Call allocates. It returns the first result, if any. Every export takes at most two
// parameters and returns at most one result.
func (i *instance) call(ctx context.Context, f api.Function, args ...uint64) (uint64, error) {
	copy(i.stack[:], args)
	if err := f.CallWithStack(ctx, i.stack[:]); err != nil {
		// Module calls never get a cancellable context, so a failed call is a trap (or a panic
		// in a host function), which may have left the shared state inconsistent.
		i.poison(err)
		return 0, err
	}
	return i.stack[0], nil
}

// call1 calls f and returns its single slice result, failing on a null slice.
func (i *instance) call1(ctx context.Context, f api.Function, args ...uint64) (uint64, error) {
	res, err := i.call(ctx, f, args...)
	if err != nil {
		return 0, err
	}
	if res == 0 {
		return 0, errors.New("module returned a null slice")
	}
	return res, nil
}

func (i *instance) unmarshal(slice uint64, m proto.Message) error {
	raceAcquire(i.race)
	defer raceRelease(i.race)
	b, ok := i.mod.Memory().Read(uint32(slice), uint32(slice>>32))
	if !ok {
		return errors.New("slice out of bounds")
	}
	return proto.Unmarshal(b, m)
}

func (i *instance) freeSlice(ctx context.Context, slice uint64) {
	_, _ = i.call(ctx, i.free, uint64(uint32(slice)))
}
