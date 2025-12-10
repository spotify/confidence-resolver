package local_resolver

import (
	"context"
	"errors"
	"runtime"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
)

type LocalResolverSupplier func() LocalResolver

type LocalResolverFactory interface {
	New() LocalResolver
	Close(context.Context) error
}

type LocalResolver interface {
	SetResolverState(*wasm.SetResolverStateRequest) error
	ResolveWithSticky(*wasm.ResolveWithStickyRequest) (*wasm.ResolveWithStickyResponse, error)
	FlushAllLogs() error
	FlushAssignLogs() error
	Close(context.Context) error
}

// DefaultResolverFactory composes the default stack: Wasm -> Recovering -> Pooled(GOMAXPROCS)
func DefaultResolverFactory(logSink LogSink) LocalResolverFactory {
	base := NewWasmResolverFactory(logSink)
	rcv := NewRecoveringResolverFactory(base)
	return NewPooledResolverFactory(rcv, runtime.GOMAXPROCS(0))
}

type localResolverImpl struct {
	PooledResolver
	factory LocalResolverFactory
}

func NewLocalResolver(ctx context.Context, logSink LogSink) LocalResolver {
	factory := NewWasmResolverFactory(logSink)
	factory = NewRecoveringResolverFactory(factory)
	return &localResolverImpl{
		PooledResolver: *NewPooledResolver(runtime.GOMAXPROCS(0), factory.New),
		factory:        factory,
	}
}

func (r *localResolverImpl) Close(ctx context.Context) error {
	err1 := r.PooledResolver.Close(ctx)
	err2 := r.factory.Close(ctx)
	return errors.Join(err1, err2)
}
