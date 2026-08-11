package local_resolver

import (
	"context"
	"fmt"
	"log/slog"
	"sync"
	"sync/atomic"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
)

// RecoveringResolverFactory composes an inner LocalResolverFactory and returns
// LocalResolver instances that auto-recover (recreate) on low-level panics.
type RecoveringResolverFactory struct {
	LocalResolverFactory
}

func NewRecoveringResolverFactory(inner LocalResolverFactory) *RecoveringResolverFactory {
	return &RecoveringResolverFactory{inner}
}

func (f *RecoveringResolverFactory) New() LocalResolver {
	rr := &RecoveringResolver{
		factory: f.LocalResolverFactory,
	}
	lr := f.LocalResolverFactory.New()
	rr.current.Store(lr)
	return rr
}

// RecoveringResolver wraps a LocalResolver and recreates it on panic.
// It also caches the last successful SetResolverState so a newly created
// resolver can be reinitialized before use.
type RecoveringResolver struct {
	factory LocalResolverFactory
	mu      sync.Mutex

	current atomic.Value // holds LocalResolver

	lastState atomic.Value // holds *wasm.SetResolverStateRequest
}

func (r *RecoveringResolver) get() LocalResolver {
	if v := r.current.Load(); v != nil {
		return v.(LocalResolver)
	}
	return nil
}

// recreateLocked swaps in a fresh resolver and closes the old one in the
// background. The caller must hold r.mu so no other operation uses old.
func (r *RecoveringResolver) recreateLocked() {
	defer func() {
		recover() // factory.New() may panic if the runtime was already closed
	}()
	old := r.get()
	newLR := r.factory.New()
	if v := r.lastState.Load(); v != nil {
		state := v.(*wasm.SetResolverStateRequest)
		_ = newLR.SetResolverState(state)
	}
	r.current.Store(newLR)
	if old != nil {
		go func() {
			_ = old.Close(context.Background())
		}()
	}
}

// withRecover ensures a resolver exists, executes fn, and sets setErr on panic or recreation failure.
func (r *RecoveringResolver) withRecover(opName string, setErr *error, fn func(LocalResolver)) {
	r.mu.Lock()
	defer r.mu.Unlock()

	defer func() {
		if rec := recover(); rec != nil {
			r.recreateLocked()
			if setErr != nil {
				*setErr = fmt.Errorf("resolver panicked during %s: %v", opName, rec)
			}
		}
	}()
	lr := r.get()
	fn(lr)
}

func (r *RecoveringResolver) SetResolverState(request *wasm.SetResolverStateRequest) (err error) {
	r.withRecover("SetResolverState", &err, func(lr LocalResolver) {
		err = lr.SetResolverState(request)
		if err == nil {
			r.lastState.Store(request)
		}
	})
	return
}

func (r *RecoveringResolver) RegisterResolve(request *wasm.RegisterResolveRequest) {
	r.mu.Lock()
	defer r.mu.Unlock()
	defer func() {
		if rec := recover(); rec != nil {
			slog.Warn("RegisterResolve panicked, ignoring", "error", rec)
		}
	}()
	lr := r.get()
	if lr == nil {
		return
	}
	lr.RegisterResolve(request)
}

func (r *RecoveringResolver) ApplyFlags(request *resolver.ApplyFlagsRequest) (err error) {
	r.withRecover("ApplyFlags", &err, func(lr LocalResolver) {
		err = lr.ApplyFlags(request)
	})
	return
}

func (r *RecoveringResolver) ResolveProcess(request *wasm.ResolveProcessRequest) (resp *wasm.ResolveProcessResponse, err error) {
	r.withRecover("ResolveProcess", &err, func(lr LocalResolver) {
		resp, err = lr.ResolveProcess(request)
	})
	return
}

func (r *RecoveringResolver) FlushAllLogs() (err error) {
	r.withRecover("FlushAllLogs", &err, func(lr LocalResolver) {
		err = lr.FlushAllLogs()
	})
	return
}

func (r *RecoveringResolver) FlushAssignLogs() (err error) {
	r.withRecover("FlushAssignLogs", &err, func(lr LocalResolver) {
		err = lr.FlushAssignLogs()
	})
	return
}

func (r *RecoveringResolver) PrometheusSnapshot(bucketsPerDecade uint32, openmetrics bool) string {
	r.mu.Lock()
	defer r.mu.Unlock()
	defer func() {
		if rec := recover(); rec != nil {
			slog.Warn("PrometheusSnapshot panicked, ignoring", "error", rec)
		}
	}()
	lr := r.get()
	if lr == nil {
		return ""
	}
	return lr.PrometheusSnapshot(bucketsPerDecade, openmetrics)
}

func (r *RecoveringResolver) Close(ctx context.Context) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	defer func() {
		recover()
	}()
	lr := r.get()
	if lr == nil {
		return nil
	}
	return lr.Close(ctx)
}
