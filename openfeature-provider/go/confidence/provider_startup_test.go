package confidence

import (
	"context"
	"sync/atomic"
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
	lr "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/local_resolver"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
	tu "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
)

type recoveringStateProvider struct {
	calls       atomic.Int32
	succeedAt   int32
	blockAfter  int32
	requestMade chan struct{}
	canceled    chan struct{}
}

type stateProviderFunc func(context.Context) ([]byte, string, error)

func (f stateProviderFunc) Provide(ctx context.Context) ([]byte, string, error) {
	return f(ctx)
}

type blockingSetResolver struct {
	mockResolverAPIForInit
	started chan struct{}
	release chan struct{}
}

func (r *blockingSetResolver) SetResolverState(*wasm.SetResolverStateRequest) error {
	close(r.started)
	<-r.release
	return nil
}

func (p *recoveringStateProvider) Provide(ctx context.Context) ([]byte, string, error) {
	call := p.calls.Add(1)
	if p.blockAfter > 0 && call >= p.blockAfter {
		close(p.requestMade)
		<-ctx.Done()
		close(p.canceled)
		return nil, "", ctx.Err()
	}
	if call < p.succeedAt {
		return nil, "", context.DeadlineExceeded
	}
	return []byte("state"), "account", nil
}

func newStartupTestProvider(stateProvider StateProvider, resolver lr.LocalResolver, opts ...Option) *LocalResolverProvider {
	return newLocalResolverProvider(
		func(context.Context, lr.LogSink) lr.LocalResolver { return resolver },
		stateProvider,
		&tu.MockFlagLogger{},
		"secret",
		nil,
		opts...,
	)
}

func waitFor(t *testing.T, timeout time.Duration, condition func() bool) {
	t.Helper()
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		if condition() {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("condition was not met before timeout")
}

func TestLocalResolverProvider_InitialStateTimeoutIsRecoverable(t *testing.T) {
	stateProvider := stateProviderFunc(func(context.Context) ([]byte, string, error) {
		time.Sleep(20 * time.Millisecond)
		return nil, "", context.DeadlineExceeded
	})
	provider := newStartupTestProvider(stateProvider, &mockResolverAPIForInit{})
	started := time.Now()

	if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatalf("Init returned a recoverable timeout: %v", err)
	}
	defer provider.Shutdown()

	if elapsed := time.Since(started); elapsed > 500*time.Millisecond {
		t.Fatalf("Init did not return after the state request timed out: %v", elapsed)
	}
	if provider.ready.Load() {
		t.Fatal("provider became ready without valid state")
	}
}

func TestLocalResolverProvider_RecoversAfterRepeatedStartupFailures(t *testing.T) {
	stateProvider := &recoveringStateProvider{succeedAt: 3}
	provider := newStartupTestProvider(
		stateProvider,
		&mockResolverAPIForInit{},
		WithStatePollInterval(20*time.Millisecond),
	)

	if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatalf("Init returned a recoverable state fetch error: %v", err)
	}
	defer provider.Shutdown()

	if provider.ready.Load() {
		t.Fatal("provider became ready without valid state")
	}

	waitFor(t, 2500*time.Millisecond, provider.ready.Load)
	waitFor(t, 500*time.Millisecond, func() bool { return stateProvider.calls.Load() >= 4 })
	if !provider.ready.Load() {
		t.Fatal("provider stopped serving the last good state after a later fetch failure")
	}
}

func TestLocalResolverProvider_ReportsRecoveryToOpenFeature(t *testing.T) {
	stateProvider := &recoveringStateProvider{succeedAt: 2}
	provider := newStartupTestProvider(stateProvider, &mockResolverAPIForInit{})
	const domain = "startup-recovery-test"

	if err := openfeature.SetNamedProvider(domain, provider); err != nil {
		t.Fatalf("SetNamedProviderAndWait returned a recoverable state fetch error: %v", err)
	}
	t.Cleanup(func() {
		provider.Shutdown()
	})

	client := openfeature.NewClient(domain)
	waitFor(t, time.Second, func() bool {
		return client.State() == openfeature.ErrorState
	})

	value, err := client.BooleanValue(context.Background(), "flag", true, openfeature.EvaluationContext{})
	if !value {
		t.Fatal("expected caller-supplied default while provider is not ready")
	}
	if err == nil {
		t.Fatal("expected evaluation error while provider is not ready")
	}

	waitFor(t, 2500*time.Millisecond, func() bool {
		return client.State() == openfeature.ReadyState
	})
}

func TestLocalResolverProvider_ReturnsDefaultWhileNotReady(t *testing.T) {
	provider := newStartupTestProvider(
		&recoveringStateProvider{succeedAt: 100},
		&mockResolverAPIForInit{},
	)

	if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatalf("Init returned a recoverable state fetch error: %v", err)
	}
	defer provider.Shutdown()

	detail := provider.BooleanEvaluation(context.Background(), "flag", true, nil)
	if !detail.Value {
		t.Fatal("expected caller-supplied default value")
	}
	if got := detail.ResolutionError.Error(); got != "PROVIDER_NOT_READY: provider not initialized" {
		t.Fatalf("expected provider-not-ready error, got %q", got)
	}
}

func TestLocalResolverProvider_ShutdownCancelsStateRetry(t *testing.T) {
	stateProvider := &recoveringStateProvider{
		succeedAt:   100,
		blockAfter:  2,
		requestMade: make(chan struct{}),
		canceled:    make(chan struct{}),
	}
	provider := newStartupTestProvider(stateProvider, &mockResolverAPIForInit{})

	if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatalf("Init returned a recoverable state fetch error: %v", err)
	}

	select {
	case <-stateProvider.requestMade:
	case <-time.After(1500 * time.Millisecond):
		t.Fatal("state retry did not start")
	}

	done := make(chan struct{})
	go func() {
		provider.Shutdown()
		close(done)
	}()

	select {
	case <-stateProvider.canceled:
	case <-time.After(time.Second):
		t.Fatal("shutdown did not cancel in-flight state request")
	}
	select {
	case <-done:
	case <-time.After(time.Second):
		t.Fatal("shutdown did not wait cleanly for state retry")
	}
}

func TestLocalResolverProvider_ShutdownCancelsInitialStateRequest(t *testing.T) {
	stateProvider := &recoveringStateProvider{
		succeedAt:   100,
		blockAfter:  1,
		requestMade: make(chan struct{}),
		canceled:    make(chan struct{}),
	}
	provider := newStartupTestProvider(stateProvider, &mockResolverAPIForInit{})
	initDone := make(chan struct{})
	go func() {
		_ = provider.Init(openfeature.EvaluationContext{})
		close(initDone)
	}()

	select {
	case <-stateProvider.requestMade:
	case <-time.After(time.Second):
		t.Fatal("initial state request did not start")
	}

	shutdownDone := make(chan struct{})
	go func() {
		provider.Shutdown()
		close(shutdownDone)
	}()

	select {
	case <-stateProvider.canceled:
	case <-time.After(time.Second):
		t.Fatal("shutdown did not cancel initial state request")
	}
	select {
	case <-initDone:
	case <-time.After(time.Second):
		t.Fatal("Init did not return after cancellation")
	}
	select {
	case <-shutdownDone:
	case <-time.After(time.Second):
		t.Fatal("shutdown did not complete after initialization stopped")
	}
}

func TestLocalResolverProvider_ShutdownWinsDuringSetResolverState(t *testing.T) {
	tests := map[string]StateProvider{
		"init": stateProviderFunc(func(context.Context) ([]byte, string, error) {
			return []byte("state"), "account", nil
		}),
		"background retry": stateProviderFunc(func() func(context.Context) ([]byte, string, error) {
			var calls atomic.Int32
			return func(context.Context) ([]byte, string, error) {
				if calls.Add(1) == 1 {
					return nil, "", context.DeadlineExceeded
				}
				return []byte("state"), "account", nil
			}
		}()),
	}

	for name, stateProvider := range tests {
		t.Run(name, func(t *testing.T) {
			resolver := &blockingSetResolver{
				started: make(chan struct{}),
				release: make(chan struct{}),
			}
			resolverContext := make(chan context.Context, 1)
			provider := newLocalResolverProvider(
				func(ctx context.Context, _ lr.LogSink) lr.LocalResolver {
					resolverContext <- ctx
					return resolver
				},
				stateProvider,
				&tu.MockFlagLogger{},
				"secret",
				nil,
			)

			initDone := make(chan error, 1)
			go func() {
				initDone <- provider.Init(openfeature.EvaluationContext{})
			}()
			ctx := <-resolverContext

			select {
			case <-resolver.started:
			case <-time.After(1500 * time.Millisecond):
				t.Fatal("SetResolverState did not start")
			}

			shutdownDone := make(chan struct{})
			go func() {
				provider.Shutdown()
				close(shutdownDone)
			}()

			select {
			case <-ctx.Done():
			case <-time.After(time.Second):
				t.Fatal("Shutdown did not cancel the resolver context")
			}
			close(resolver.release)

			select {
			case err := <-initDone:
				if err != nil {
					t.Fatalf("Init returned an error: %v", err)
				}
			case <-time.After(time.Second):
				t.Fatal("Init did not complete")
			}
			select {
			case <-shutdownDone:
			case <-time.After(time.Second):
				t.Fatal("Shutdown did not complete")
			}

			if provider.ready.Load() {
				t.Fatal("provider became ready after Shutdown")
			}
			for {
				select {
				case event := <-provider.EventChannel():
					if event.EventType == openfeature.ProviderReady {
						t.Fatal("provider reported ready after Shutdown")
					}
				default:
					return
				}
			}
		})
	}
}
