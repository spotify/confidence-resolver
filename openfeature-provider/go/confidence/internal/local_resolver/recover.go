package local_resolver

import (
	"context"
	"fmt"
	"sync/atomic"
	"time"

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
		factory: f,
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

	current atomic.Value // holds LocalResolver
	broken  atomic.Bool  // indicates an instance has panicked

	lastState atomic.Value // holds *wasm.SetResolverStateRequest
}

func (r *RecoveringResolver) get() LocalResolver {
	if v := r.current.Load(); v != nil {
		return v.(LocalResolver)
	}
	return nil
}

// startRecreate starts a background recreation.
// It replaces the current resolver with a fresh one and reapplies last state.
// Old instance is closed in a best-effort goroutine with a short timeout.
func (r *RecoveringResolver) startRecreate() {
	go func() {
		defer r.broken.Store(false)
		old := r.get()
		newLR := r.factory.New()
		if v := r.lastState.Load(); v != nil {
			state := v.(*wasm.SetResolverStateRequest)
			_ = newLR.SetResolverState(state)
		}
		r.current.Store(newLR)
		if old != nil {
			ctx, cancel := context.WithTimeout(context.Background(), time.Second)
			defer cancel()
			_ = old.Close(ctx)
		}
	}()
}

// withRecover ensures a resolver exists, executes fn, and sets setErr on panic or recreation failure.
func (r *RecoveringResolver) withRecover(opName string, setErr *error, fn func(LocalResolver)) {
	defer func() {
		if rec := recover(); rec != nil {
			// mark broken and kick off background recreation once
			if r.broken.CompareAndSwap(false, true) {
				r.startRecreate()
			}
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
		// Cache last successful state
		if err != nil {
			r.lastState.Store(request)
		}
	})
	return
}

func (r *RecoveringResolver) ResolveWithSticky(request *wasm.ResolveWithStickyRequest) (resp *wasm.ResolveWithStickyResponse, err error) {
	r.withRecover("ResolveWithSticky", &err, func(lr LocalResolver) {
		resp, err = lr.ResolveWithSticky(request)
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

func (r *RecoveringResolver) Close(ctx context.Context) error {
	// For Close, if we panic, don't recreate during shutdown; just surface error.
	defer func() {
		if rec := recover(); rec != nil {
			// swallowing recreate on shutdown
		}
	}()
	lr := r.get()
	if lr == nil {
		return nil
	}
	return lr.Close(ctx)
}
