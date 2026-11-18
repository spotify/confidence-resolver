// Package engine owns the long-lived parts of a provider: the module fetcher, the log shipper, the
// dedupe filters, and the current module generation. It swaps generations as new modules are
// published.
package engine

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"sync"
	"sync/atomic"
	"time"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/cdn"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/host"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/ingest"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/s2wpb/resolverpb"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/s2wpb/wasmpb"
)

var (
	// ErrNotReady: no module has been installed yet.
	ErrNotReady = errors.New("provider not ready: no resolver module loaded yet")
	// ErrClosed: the engine has been closed.
	ErrClosed = errors.New("provider is closed")
)

// ExposureMode selects how exposure (apply) events are deduplicated.
type ExposureMode int

const (
	ExposureDeduplicated ExposureMode = iota // default
	ExposureAll                              // no dedup
	ExposureNone                             // suppress non-error exposure events
)

// Filter sizes, as in the Java host.
const (
	applyFilterPages  = 8
	schemaFilterPages = 2
)

// Config configures an Engine. Zero values select defaults.
type Config struct {
	ClientSecret  string
	EncryptionKey []byte // 32 bytes; see cdn.ParseKey
	HTTPClient    *http.Client
	CDNBaseURL    string
	IngestBaseURL string

	Instances      int
	MaxMemoryBytes uint64
	Interpreter    bool
	Exposure       ExposureMode

	PollInterval  time.Duration // default 10s
	FlushInterval time.Duration // default 30s
	RetryInterval time.Duration // startup retry; default 1s

	Logger *slog.Logger
	// OnReady is called once, when the first module is installed after Start returned without one.
	OnReady func()
}

// Engine evaluates against the current module generation.
type Engine struct {
	cfg          Config
	fetcher      *cdn.Fetcher
	shipper      *ingest.Shipper
	applyFilter  host.Filter
	schemaFilter host.Filter
	closeFilters []func(context.Context) error

	current  atomic.Pointer[generation]
	closed   atomic.Bool
	stop     context.CancelFunc
	loops    sync.WaitGroup
	retiring sync.WaitGroup
	notified atomic.Bool
}

// generation is one installed module. Requests hold mu for reading for the whole evaluation. To
// retire a generation, first replace the engine's pointer, then take mu for writing and never
// release it: Lock waits for readers, and every later TryRLock fails, sending the request back to
// reload the pointer, which by then points elsewhere.
type generation struct {
	mu       sync.RWMutex
	resolver *host.Resolver
	etag     string
}

// New creates an engine. It doesn't touch the network; call Start.
func New(ctx context.Context, cfg Config) (*Engine, error) {
	if cfg.PollInterval <= 0 {
		cfg.PollInterval = 10 * time.Second
	}
	if cfg.FlushInterval <= 0 {
		cfg.FlushInterval = 30 * time.Second
	}
	if cfg.RetryInterval <= 0 {
		cfg.RetryInterval = time.Second
	}
	if cfg.Logger == nil {
		cfg.Logger = slog.Default()
	}
	if cfg.OnReady == nil {
		cfg.OnReady = func() {}
	}
	fetcher, err := cdn.NewFetcher(cfg.ClientSecret, cfg.EncryptionKey, cfg.CDNBaseURL, cfg.HTTPClient)
	if err != nil {
		return nil, err
	}
	e := &Engine{cfg: cfg, fetcher: fetcher}
	switch cfg.Exposure {
	case ExposureAll:
		e.applyFilter = host.NeverSeen
	case ExposureNone:
		e.applyFilter = host.AlwaysSeen
	default:
		if e.applyFilter, err = e.dedupeFilter(ctx, applyFilterPages); err != nil {
			return nil, err
		}
	}
	if e.schemaFilter, err = e.dedupeFilter(ctx, schemaFilterPages); err != nil {
		e.closeAllFilters(ctx)
		return nil, err
	}
	e.shipper = ingest.New(ingest.Config{
		BaseURL: cfg.IngestBaseURL, Client: cfg.HTTPClient,
		Report: func(err error) { cfg.Logger.Warn("flag log delivery failed", "error", err) },
	})
	return e, nil
}

func (e *Engine) dedupeFilter(ctx context.Context, pages uint32) (host.Filter, error) {
	f, err := host.NewDedupeFilter(ctx, pages, e.cfg.Interpreter)
	if err != nil {
		return nil, err
	}
	e.closeFilters = append(e.closeFilters, f.Close)
	return f, nil
}

func (e *Engine) closeAllFilters(ctx context.Context) error {
	var errs []error
	for _, c := range e.closeFilters {
		errs = append(errs, c(ctx))
	}
	return errors.Join(errs...)
}

// TerminalError is a startup failure that retrying won't fix.
type TerminalError struct{ Err error }

func (e *TerminalError) Error() string { return e.Err.Error() }
func (e *TerminalError) Unwrap() error { return e.Err }

// Start installs the first module, retrying until initTimeout, then starts polling and timed log
// flushes. A TerminalError means the engine won't become ready and is not polling. Any other error
// means the first module didn't arrive in time: the engine keeps polling, and calls OnReady once
// one is installed.
func (e *Engine) Start(ctx context.Context, initTimeout time.Duration) error {
	loopCtx, stop := context.WithCancel(context.Background())
	e.stop = stop
	startCtx, cancel := context.WithTimeout(ctx, initTimeout)
	defer cancel()
	var lastErr error
	for {
		err := e.load(startCtx)
		if err == nil {
			e.notified.Store(true) // Start reports readiness itself
			break
		}
		var term *TerminalError
		if errors.As(err, &term) {
			stop()
			return err
		}
		lastErr = err
		e.cfg.Logger.Warn("loading resolver module failed; retrying", "error", err)
		select {
		case <-startCtx.Done():
			e.startLoops(loopCtx)
			return fmt.Errorf("no resolver module within %s (serving defaults, still polling): %w", initTimeout, lastErr)
		case <-time.After(e.cfg.RetryInterval):
		}
	}
	e.startLoops(loopCtx)
	return nil
}

func (e *Engine) startLoops(ctx context.Context) {
	e.loops.Add(2)
	go e.every(ctx, e.cfg.PollInterval, func() {
		if err := e.load(ctx); err != nil {
			e.cfg.Logger.Warn("updating resolver module failed; keeping the current one", "error", err)
		}
	})
	go e.every(ctx, e.cfg.FlushInterval, e.flush)
}

func (e *Engine) every(ctx context.Context, d time.Duration, f func()) {
	defer e.loops.Done()
	t := time.NewTicker(d)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-t.C:
			f()
		}
	}
}

// load fetches the module and, if it changed, installs it as a new generation.
func (e *Engine) load(ctx context.Context) error {
	m, err := e.fetcher.Fetch(ctx)
	switch {
	case errors.Is(err, cdn.ErrNotModified):
		return nil
	case err != nil:
		var term *cdn.TerminalError
		if errors.As(err, &term) {
			return &TerminalError{err}
		}
		return err
	}
	token := m.LogToken
	r, err := host.NewResolver(ctx, m.Wasm, e.cfg.Interpreter, host.Config{
		Instances:      e.cfg.Instances,
		MaxMemoryBytes: e.cfg.MaxMemoryBytes,
		ApplyFilter:    e.applyFilter,
		SchemaFilter:   e.schemaFilter,
		WriteLogs: func(b []byte) {
			if !e.shipper.Enqueue(b, token) {
				e.cfg.Logger.Warn("flag log queue full; dropping a batch")
			}
		},
		ReportFailure: func(err error) { e.cfg.Logger.Warn("resolver host callback failed", "error", err) },
	})
	if err != nil {
		if errors.Is(err, host.ErrInvalidModule) {
			return &TerminalError{err}
		}
		return err
	}
	if err := r.Warm(ctx); err != nil {
		_ = r.Close(ctx)
		return fmt.Errorf("warming up module: %w", err)
	}
	if e.closed.Load() {
		return r.Close(ctx)
	}
	old := e.current.Swap(&generation{resolver: r, etag: m.ETag})
	e.fetcher.Accept(m.ETag)
	if old != nil {
		e.retiring.Add(1)
		go func() {
			defer e.retiring.Done()
			e.retire(old)
		}()
	} else if !e.notified.Swap(true) {
		e.cfg.OnReady()
	}
	return nil
}

// retire drains g (which must already be unpublished), flushes its logs and closes it.
func (e *Engine) retire(g *generation) {
	g.mu.Lock() // never released
	if err := g.resolver.Close(context.Background()); err != nil {
		e.cfg.Logger.Warn("closing retired resolver module", "error", err)
	}
}

// acquire returns the current generation, read-locked. The caller must RUnlock it.
func (e *Engine) acquire() (*generation, error) {
	for {
		g := e.current.Load()
		if g == nil {
			if e.closed.Load() {
				return nil, ErrClosed
			}
			return nil, ErrNotReady
		}
		// One of the rare correct uses of TryRLock: failure means g is being retired and has
		// already been replaced, so reloading makes progress.
		if g.mu.TryRLock() {
			return g, nil
		}
	}
}

// Evaluate resolves req and evaluates evals against the current module.
func (e *Engine) Evaluate(ctx context.Context, req *resolverpb.ResolveFlagsRequest, evals []*wasmpb.EvaluateRequest, store host.MaterializationStore) ([]*wasmpb.ResolutionDetails, error) {
	g, err := e.acquire()
	if err != nil {
		return nil, err
	}
	defer g.mu.RUnlock()
	return g.resolver.Evaluate(ctx, req, evals, store)
}

// Resolve resolves req against the current module and returns its full response.
func (e *Engine) Resolve(ctx context.Context, req *resolverpb.ResolveFlagsRequest, store host.MaterializationStore) (*resolverpb.ResolveFlagsResponse, error) {
	g, err := e.acquire()
	if err != nil {
		return nil, err
	}
	defer g.mu.RUnlock()
	return g.resolver.Resolve(ctx, req, store)
}

// Metrics renders the current module's cumulative telemetry ("" before the first module).
func (e *Engine) Metrics(ctx context.Context, openMetrics bool) (string, error) {
	g, err := e.acquire()
	if err != nil {
		return "", nil
	}
	defer g.mu.RUnlock()
	return g.resolver.Metrics(ctx, openMetrics)
}

// ApplyFlags forwards a serialized ApplyFlagsRequest to the current module.
func (e *Engine) ApplyFlags(ctx context.Context, req []byte) error {
	g, err := e.acquire()
	if err != nil {
		return err
	}
	defer g.mu.RUnlock()
	return g.resolver.ApplyFlags(ctx, req)
}

func (e *Engine) flush() {
	g, err := e.acquire()
	if err != nil {
		return
	}
	defer g.mu.RUnlock()
	if err := g.resolver.FlushLogs(context.Background()); err != nil {
		e.cfg.Logger.Warn("flushing flag logs", "error", err)
	}
}

// Close stops polling, retires the current module (flushing its logs) and waits for the log queue
// to drain, bounded by ctx. Retiring waits for in-flight evaluations, which may be in a slow
// materialization read; if ctx ends first, Close returns its error and the rest of the cleanup
// runs once they finish. New evaluations fail with ErrClosed either way.
func (e *Engine) Close(ctx context.Context) error {
	if e.closed.Swap(true) {
		return nil
	}
	if e.stop != nil {
		e.stop()
	}
	e.loops.Wait()
	if g := e.current.Swap(nil); g != nil {
		e.retiring.Add(1)
		go func() {
			defer e.retiring.Done()
			e.retire(g)
		}()
	}
	retired := make(chan struct{})
	go func() {
		e.retiring.Wait()
		close(retired)
	}()
	select {
	case <-retired:
		// The filters and the shipper outlive every generation: modules link to the filters and
		// flush their logs into the shipper when retired.
		return errors.Join(e.shipper.Close(ctx), e.closeAllFilters(ctx))
	case <-ctx.Done():
		go func() {
			<-retired
			if err := errors.Join(e.shipper.Close(context.Background()), e.closeAllFilters(context.Background())); err != nil {
				e.cfg.Logger.Warn("closing after in-flight evaluations finished", "error", err)
			}
		}()
		return fmt.Errorf("evaluations still in flight: %w", ctx.Err())
	}
}
