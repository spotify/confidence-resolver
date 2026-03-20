package local_resolver

import (
	"context"
	"errors"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
)

const DefaultPoolSize = 2

type LocalResolverSupplier func() LocalResolver

type LocalResolverFactory interface {
	New() LocalResolver
	Close(context.Context) error
}

type LocalResolver interface {
	SetResolverState(*wasm.SetResolverStateRequest) error
	ResolveProcess(*wasm.ResolveProcessRequest) (*wasm.ResolveProcessResponse, error)
	ApplyFlags(*resolver.ApplyFlagsRequest) error
	FlushAllLogs() error
	FlushAssignLogs() error
	TrackEvent(*wasm.Event) error
	FlushEvents() (*wasm.FlushEventsResponse, error)
	Close(context.Context) error
}

// DefaultResolverFactory composes the default stack: Wasm -> Recovering -> Pooled(DefaultPoolSize)
func DefaultResolverFactory(logSink LogSink) LocalResolverFactory {
	base := NewWasmResolverFactory(logSink)
	rcv := NewRecoveringResolverFactory(base)
	return NewPooledResolverFactory(rcv, DefaultPoolSize)
}

type localResolverImpl struct {
	PooledResolver
	factory LocalResolverFactory
}

func NewLocalResolverWithPoolSize(ctx context.Context, logSink LogSink, poolSize int) LocalResolver {
	factory := NewWasmResolverFactory(logSink)
	factory = NewRecoveringResolverFactory(factory)
	if poolSize <= 0 {
		poolSize = DefaultPoolSize
	}
	return &localResolverImpl{
		PooledResolver: *NewPooledResolver(poolSize, factory.New),
		factory:        factory,
	}
}

func (r *localResolverImpl) Close(ctx context.Context) error {
	err1 := r.PooledResolver.Close(ctx)
	err2 := r.factory.Close(ctx)
	return errors.Join(err1, err2)
}
