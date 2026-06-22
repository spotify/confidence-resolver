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
// WASM linear memory can grow but never shrink (spec limitation). Repeated
// state updates on a long-lived instance cause heap fragmentation from
// interleaved resolve allocations, leading to unbounded memory.grow calls.
// Replacing instances reclaims the old linear memory entirely.
func (s *PooledResolver) SetResolverState(request *wasm.SetResolverStateRequest) error {
	s.mmu.Lock()
	defer s.mmu.Unlock()

	var errs []error
	for i := range s.slots {
		slot := &s.slots[i]

		fresh := s.supplier()
		if err := fresh.SetResolverState(request); err != nil {
			fresh.Close(context.Background())
			errs = append(errs, fmt.Errorf("slot %d: %w", i, err))
			continue
		}

		slot.rw.Lock()
		old := slot.lr
		slot.lr = fresh
		slot.rw.Unlock()

		old.Close(context.Background())
	}

	if len(errs) > 0 {
		return errors.Join(errs...)
	}
	return nil
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
