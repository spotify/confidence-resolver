package local_resolver

import (
	"context"
	"errors"
	"fmt"
	"sync"
	"sync/atomic"

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

// ResolveWithSticky implements LocalResolver.
func (s *PooledResolver) ResolveWithSticky(request *wasm.ResolveWithStickyRequest) (*wasm.ResolveWithStickyResponse, error) {
	n := uint64(len(s.slots))
	idx := s.rr.Add(1)
	for !s.slots[idx%n].rw.TryRLock() {
		idx = s.rr.Add(1)
	}
	slot := &s.slots[idx%n]
	defer slot.rw.RUnlock()
	return slot.lr.ResolveWithSticky(request)
}

// SetResolverState implements LocalResolver.
func (s *PooledResolver) SetResolverState(request *wasm.SetResolverStateRequest) error {
	return s.maintenance(func(lr LocalResolver) error {
		return lr.SetResolverState(request)
	})
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
