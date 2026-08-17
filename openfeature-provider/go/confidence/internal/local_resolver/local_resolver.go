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
	InitLabels         map[string]string
	InitSDK            *resolver.Sdk
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
	base := NewWasmResolverFactoryWithLabels(logSink, cfg.UseWasmInterpreter, cfg.InitLabels, cfg.InitSDK)
	rcv := NewRecoveringResolverFactory(base)
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

func NewLocalResolverWithPoolSize(ctx context.Context, logSink LogSink, poolSize int) LocalResolver {
	return NewLocalResolver(ctx, logSink, LocalResolverConfig{PoolSize: poolSize})
}

func NewLocalResolver(ctx context.Context, logSink LogSink, cfg LocalResolverConfig) LocalResolver {
	return newLocalResolver(ctx, logSink, cfg, cfg.InitLabels)
}

func NewLocalResolverWithLabels(ctx context.Context, logSink LogSink, cfg LocalResolverConfig, initLabels map[string]string) LocalResolver {
	return newLocalResolver(ctx, logSink, cfg, initLabels)
}

func newLocalResolver(ctx context.Context, logSink LogSink, cfg LocalResolverConfig, initLabels map[string]string) LocalResolver {
	poolSize := cfg.PoolSize
	if poolSize <= 0 {
		poolSize = DefaultPoolSize
	}
	var factory LocalResolverFactory
	if len(initLabels) > 0 || cfg.InitSDK != nil {
		factory = NewWasmResolverFactoryWithLabels(logSink, cfg.UseWasmInterpreter, initLabels, cfg.InitSDK)
	} else {
		factory = NewWasmResolverFactory(logSink, cfg.UseWasmInterpreter)
	}
	factory = NewRecoveringResolverFactory(factory)
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
