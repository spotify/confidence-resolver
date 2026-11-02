package local_resolver

import (
	"context"
	"errors"
	"fmt"
	"runtime"
	"strings"
	"sync"
	"sync/atomic"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
)

type PooledResolverFactory struct {
	size  int
	inner LocalResolverFactory
}

func NewPooledResolverFactory(inner LocalResolverFactory, size int) LocalResolverFactory {
	return &PooledResolverFactory{
		size:  size,
		inner: inner,
	}
}

func (f *PooledResolverFactory) New() LocalResolver {
	return NewPooledResolver(f.size, f.inner.New)
}

func (f *PooledResolverFactory) Close(ctx context.Context) error {
	return f.inner.Close(ctx)
}

type slot struct {
	lr LocalResolver
	rw *sync.RWMutex
	// stateLoaded is true once lr holds a resolver state.
	stateLoaded bool
}

type PooledResolver struct {
	supplier LocalResolverSupplier
	slots    []slot
	rr       atomic.Uint64
	mmu      sync.Mutex
}

var _ LocalResolver = (*PooledResolver)(nil)

func NewPooledResolver(size int, supplier LocalResolverSupplier) *PooledResolver {
	maxProcs := runtime.GOMAXPROCS(0)
	if size > maxProcs {
		size = maxProcs
	}
	slots := make([]slot, size+1)
	for i := range slots {
		slots[i] = slot{
			lr: supplier(),
			rw: &sync.RWMutex{},
		}
	}
	return &PooledResolver{
		supplier: supplier,
		slots:    slots,
	}
}

// ResolveProcess implements LocalResolver.
func (s *PooledResolver) ResolveProcess(request *wasm.ResolveProcessRequest) (*wasm.ResolveProcessResponse, error) {
	n := uint64(len(s.slots))
	idx := s.rr.Add(1)
	for !s.slots[idx%n].rw.TryRLock() {
		idx = s.rr.Add(1)
	}
	slot := &s.slots[idx%n]
	defer slot.rw.RUnlock()
	return slot.lr.ResolveProcess(request)
}

// RegisterResolve implements LocalResolver.
func (s *PooledResolver) RegisterResolve(request *wasm.RegisterResolveRequest) {
	n := uint64(len(s.slots))
	idx := s.rr.Add(1)
	for !s.slots[idx%n].rw.TryRLock() {
		idx = s.rr.Add(1)
	}
	slot := &s.slots[idx%n]
	defer slot.rw.RUnlock()
	slot.lr.RegisterResolve(request)
}

// ApplyFlags implements LocalResolver.
func (s *PooledResolver) ApplyFlags(request *resolver.ApplyFlagsRequest) error {
	n := uint64(len(s.slots))
	idx := s.rr.Add(1)
	for !s.slots[idx%n].rw.TryRLock() {
		idx = s.rr.Add(1)
	}
	slot := &s.slots[idx%n]
	defer slot.rw.RUnlock()
	return slot.lr.ApplyFlags(request)
}

// SetResolverState implements LocalResolver.
//
// A slot that already holds a state is replaced by a fresh instance loaded
// with the new state instead of being updated in place. WASM linear memory
// never shrinks, so an in-place update leaves the instance sized for the old
// state, the encoded request and the new state at once, and later updates
// fragment the freed space (#455, #665). The old instance flushes its pending logs
// when it is closed. A slot whose fresh instance fails to load keeps serving
// the old state.
func (s *PooledResolver) SetResolverState(request *wasm.SetResolverStateRequest) error {
	errs := []error{}
	s.mmu.Lock()
	defer s.mmu.Unlock()
	for i := range s.slots {
		if err := s.loadState(&s.slots[i], request); err != nil {
			errs = append(errs, fmt.Errorf("slot %d: %w", i, err))
		}
	}
	if len(errs) > 0 {
		return errors.Join(errs...)
	}
	return nil
}

func (s *PooledResolver) loadState(slot *slot, request *wasm.SetResolverStateRequest) error {
	slot.rw.Lock()
	defer slot.rw.Unlock()
	if !slot.stateLoaded {
		if err := slot.lr.SetResolverState(request); err != nil {
			return err
		}
		slot.stateLoaded = true
		return nil
	}
	fresh, err := s.newInstance()
	if err != nil {
		return err
	}
	if err := fresh.SetResolverState(request); err != nil {
		_ = fresh.Close(context.Background())
		return err
	}
	old := slot.lr
	slot.lr = fresh
	_ = old.Close(context.Background())
	return nil
}

// newInstance calls the supplier, turning its panic (how the WASM factory
// reports an instantiation failure) into an error.
func (s *PooledResolver) newInstance() (lr LocalResolver, err error) {
	defer func() {
		if rec := recover(); rec != nil {
			err = fmt.Errorf("creating resolver instance: %v", rec)
		}
	}()
	return s.supplier(), nil
}

// FlushAllLogs implements LocalResolver.
func (s *PooledResolver) FlushAllLogs() error {
	return s.maintenance(func(lr LocalResolver) error {
		return lr.FlushAllLogs()
	})
}

// FlushAssignLogs implements LocalResolver.
func (s *PooledResolver) FlushAssignLogs() error {
	return s.maintenance(func(lr LocalResolver) error {
		return lr.FlushAssignLogs()
	})
}

// PrometheusSnapshot implements LocalResolver.
// Outputs from multiple pool instances are concatenated with duplicate
// # TYPE and # HELP meta lines removed so the result is valid Prometheus
// exposition text.
func (s *PooledResolver) PrometheusSnapshot(bucketsPerDecade uint32, openmetrics bool) string {
	var fragments []string
	s.maintenance(func(lr LocalResolver) error {
		text := lr.PrometheusSnapshot(bucketsPerDecade, openmetrics)
		if text != "" {
			fragments = append(fragments, text)
		}
		return nil
	})
	if len(fragments) <= 1 {
		if len(fragments) == 1 {
			return fragments[0]
		}
		return ""
	}
	return deduplicateMetaLines(fragments)
}

// deduplicateMetaLines merges multiple Prometheus/OpenMetrics text fragments,
// keeping only the first occurrence of each # TYPE / # HELP line.
// Any # EOF lines are stripped from fragments and a single # EOF is appended
// at the end if one was present (required for OpenMetrics).
func deduplicateMetaLines(fragments []string) string {
	seen := make(map[string]bool)
	hasEOF := false
	var b strings.Builder
	for _, frag := range fragments {
		for _, line := range strings.Split(frag, "\n") {
			if line == "# EOF" {
				hasEOF = true
				continue
			}
			if strings.HasPrefix(line, "# ") {
				if seen[line] {
					continue
				}
				seen[line] = true
			}
			b.WriteString(line)
			b.WriteByte('\n')
		}
	}
	if hasEOF {
		b.WriteString("# EOF\n")
	}
	return b.String()
}

func (s *PooledResolver) Close(ctx context.Context) error {
	return s.maintenance(func(lr LocalResolver) error {
		return lr.Close(ctx)
	})
}

func (s *PooledResolver) maintenance(fn func(LocalResolver) error) error {
	errs := []error{}
	s.mmu.Lock()
	defer s.mmu.Unlock()
	for i, slot := range s.slots {
		func() {
			slot.rw.Lock()
			defer slot.rw.Unlock()
			if err := fn(slot.lr); err != nil {
				errs = append(errs, fmt.Errorf("slot %d: %w", i, err))
			}
		}()
	}
	if len(errs) > 0 {
		return errors.Join(errs...)
	}
	return nil
}
