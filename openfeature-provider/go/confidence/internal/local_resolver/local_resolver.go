package local_resolver

import (
	"context"
	"errors"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
)

const DefaultPoolSize = 2

// LocalResolverConfig configures the local WASM resolver stack.
type LocalResolverConfig struct {
	PoolSize           int
	UseWasmInterpreter bool
	Logger             Logger
}

type LocalResolverSupplier func() LocalResolver

type LocalResolverFactory interface {
	New() LocalResolver
	Close(context.Context) error
}

type LocalResolver interface {
	SetResolverState(*wasm.SetResolverStateRequest) error
	ResolveProcess(*wasm.ResolveProcessRequest) (*wasm.ResolveProcessResponse, error)
	RegisterResolve(*wasm.RegisterResolveRequest)
	ApplyFlags(*resolver.ApplyFlagsRequest) error
	FlushAllLogs() error
	FlushAssignLogs() error
	// PrometheusSnapshot returns Prometheus/OpenMetrics text-format metrics.
	// bucketsPerDecade controls histogram bucket density (1-18, 0 = default 18).
	// openmetrics switches output to OpenMetrics text format.
	PrometheusSnapshot(bucketsPerDecade uint32, openmetrics bool) string
	Close(context.Context) error
}

// DefaultResolverFactory composes the default stack: Wasm -> Recovering -> Pooled(DefaultPoolSize)
func DefaultResolverFactory(logSink LogSink, cfg LocalResolverConfig) LocalResolverFactory {
	logger := defaultLogger(cfg.Logger)
	base := NewWasmResolverFactory(logSink, logger, cfg.UseWasmInterpreter)
	rcv := NewRecoveringResolverFactory(base, logger)
	poolSize := cfg.PoolSize
	if poolSize <= 0 {
		poolSize = DefaultPoolSize
	}
	return NewPooledResolverFactory(rcv, poolSize)
}

type localResolverImpl struct {
	PooledResolver
	factory LocalResolverFactory
}

func NewLocalResolverWithPoolSize(ctx context.Context, logSink LogSink, logger Logger, poolSize int) LocalResolver {
	return NewLocalResolver(ctx, logSink, LocalResolverConfig{PoolSize: poolSize, Logger: logger})
}

func NewLocalResolver(ctx context.Context, logSink LogSink, cfg LocalResolverConfig) LocalResolver {
	logger := defaultLogger(cfg.Logger)
	poolSize := cfg.PoolSize
	if poolSize <= 0 {
		poolSize = DefaultPoolSize
	}
	factory := NewWasmResolverFactory(logSink, logger, cfg.UseWasmInterpreter)
	factory = NewRecoveringResolverFactory(factory, logger)
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
