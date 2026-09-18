package confidence

import (
	"context"
	"errors"
	"sync/atomic"
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
	"github.com/open-feature/go-sdk/openfeature/isolated"
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

func TestLocalResolverProvider_InitializationTimeoutConfig(t *testing.T) {
	provider := newStartupTestProvider(nil, nil)
	if provider.initializationTimeout != defaultInitializationTimeout {
		t.Fatalf("default initialization timeout = %v, want %v", provider.initializationTimeout, defaultInitializationTimeout)
	}

	const configured = 17 * time.Millisecond
	opts := buildProviderOptions(0, 0, configured, false, false)
	provider = newStartupTestProvider(nil, nil, opts...)
	if provider.initializationTimeout != configured {
		t.Fatalf("configured initialization timeout = %v, want %v", provider.initializationTimeout, configured)
	}
}

func TestLocalResolverProvider_RetriesWithinStartupBudget(t *testing.T) {
	stateProvider := &recoveringStateProvider{succeedAt: 3}
	provider := newStartupTestProvider(
		stateProvider,
		&mockResolverAPIForInit{},
		WithInitializationTimeout(200*time.Millisecond),
		withInitialStateRetryInterval(5*time.Millisecond),
	)
	defer provider.Shutdown()

	if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatalf("Init returned an error after startup recovery: %v", err)
	}
	if !provider.ready.Load() {
		t.Fatal("provider did not become ready after valid state was installed")
	}
	if got := stateProvider.calls.Load(); got != 3 {
		t.Fatalf("state fetch attempts = %d, want 3", got)
	}
}

func TestLocalResolverProvider_StartupBudgetIsTotalDeadline(t *testing.T) {
	var deadlines atomic.Int32
	stateProvider := stateProviderFunc(func(ctx context.Context) ([]byte, string, error) {
		if deadline, ok := ctx.Deadline(); ok && time.Until(deadline) <= 50*time.Millisecond {
			deadlines.Add(1)
		}
		return nil, "", context.DeadlineExceeded
	})
	provider := newStartupTestProvider(
		stateProvider,
		&mockResolverAPIForInit{},
		WithInitializationTimeout(30*time.Millisecond),
		withInitialStateRetryInterval(5*time.Millisecond),
	)
	defer provider.Shutdown()

	started := time.Now()
	err := provider.Init(openfeature.EvaluationContext{})
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("Init error = %v, want recoverable deadline exceeded", err)
	}
	if elapsed := time.Since(started); elapsed > 250*time.Millisecond {
		t.Fatalf("startup exceeded its total budget: %v", elapsed)
	}
	if deadlines.Load() < 2 {
		t.Fatal("expected retries to share the startup deadline")
	}
	if provider.ready.Load() {
		t.Fatal("provider became ready without valid state")
	}
}

func TestLocalResolverProvider_OpenFeatureRegistrationAndRecovery(t *testing.T) {
	for _, synchronous := range []bool{true, false} {
		for _, named := range []bool{false, true} {
			name := "async/default"
			if synchronous {
				name = "sync/default"
			}
			if named {
				name = name[:len(name)-len("default")] + "named"
			}
			t.Run(name, func(t *testing.T) {
				var recoverNow atomic.Bool
				stateProvider := stateProviderFunc(func(context.Context) ([]byte, string, error) {
					if recoverNow.Load() {
						return []byte("state"), "account", nil
					}
					return nil, "", context.DeadlineExceeded
				})
				provider := newStartupTestProvider(
					stateProvider,
					&mockResolverAPIForInit{},
					WithInitializationTimeout(25*time.Millisecond),
					withInitialStateRetryInterval(5*time.Millisecond),
				)
				api := isolated.NewAPI()
				t.Cleanup(func() { _ = api.Shutdown(context.Background()) })

				readyEvents := make(chan bool, 2)
				readyHandler := func(openfeature.EventDetails) {
					readyEvents <- provider.ready.Load()
				}
				api.AddHandler(openfeature.ProviderReady, &readyHandler)

				var opts []openfeature.APIOption
				if named {
					opts = append(opts, openfeature.WithDomain(t.Name()))
				}
				var err error
				if synchronous {
					err = api.SetProviderAndWait(context.Background(), provider, opts...)
					if !errors.Is(err, context.DeadlineExceeded) {
						t.Fatalf("SetProviderAndWait error = %v, want deadline exceeded", err)
					}
				} else if err = api.SetProvider(context.Background(), provider, opts...); err != nil {
					t.Fatalf("SetProvider returned an error: %v", err)
				}

				client := api.NewClient(opts...)
				waitFor(t, time.Second, func() bool { return client.State() == openfeature.ErrorState })
				select {
				case actuallyReady := <-readyEvents:
					t.Fatalf("premature PROVIDER_READY callback fired with internal ready=%v", actuallyReady)
				default:
				}

				value, evalErr := client.BooleanValue(context.Background(), "flag", true, openfeature.EvaluationContext{})
				if !value || evalErr == nil {
					t.Fatalf("evaluation while recovering = (%v, %v), want caller default and error", value, evalErr)
				}

				recoverNow.Store(true)
				select {
				case actuallyReady := <-readyEvents:
					if !actuallyReady {
						t.Fatal("PROVIDER_READY fired before valid state was installed")
					}
				case <-time.After(time.Second):
					t.Fatal("provider did not report background recovery")
				}
				waitFor(t, time.Second, func() bool { return client.State() == openfeature.ReadyState })
			})
		}
	}
}

func TestLocalResolverProvider_RecoveryResumesStatePollInterval(t *testing.T) {
	requests := make(chan struct{}, 16)
	var recoverNow atomic.Bool
	stateProvider := stateProviderFunc(func(context.Context) ([]byte, string, error) {
		requests <- struct{}{}
		if recoverNow.Load() {
			return []byte("state"), "account", nil
		}
		return nil, "", context.DeadlineExceeded
	})
	provider := newStartupTestProvider(
		stateProvider,
		&mockResolverAPIForInit{},
		WithInitializationTimeout(10*time.Millisecond),
		withInitialStateRetryInterval(5*time.Millisecond),
		WithStatePollInterval(80*time.Millisecond),
	)
	defer provider.Shutdown()

	if err := provider.Init(openfeature.EvaluationContext{}); !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("Init error = %v, want deadline exceeded", err)
	}
	recoverNow.Store(true)
	select {
	case event := <-provider.EventChannel():
		if event.EventType != openfeature.ProviderReady {
			t.Fatalf("recovery event = %s, want %s", event.EventType, openfeature.ProviderReady)
		}
	case <-time.After(time.Second):
		t.Fatal("provider did not recover")
	}
	for len(requests) > 0 {
		<-requests
	}

	select {
	case <-requests:
		t.Fatal("provider kept using the startup retry interval after recovery")
	case <-time.After(30 * time.Millisecond):
	}
	select {
	case <-requests:
	case <-time.After(150 * time.Millisecond):
		t.Fatal("provider did not resume the configured state poll interval")
	}
}

func TestLocalResolverProvider_ShutdownCancelsStateRetry(t *testing.T) {
	stateProvider := &recoveringStateProvider{
		succeedAt:   100,
		blockAfter:  2,
		requestMade: make(chan struct{}),
		canceled:    make(chan struct{}),
	}
	provider := newStartupTestProvider(
		stateProvider,
		&mockResolverAPIForInit{},
		WithInitializationTimeout(10*time.Millisecond),
		withInitialStateRetryInterval(20*time.Millisecond),
	)

	if err := provider.Init(openfeature.EvaluationContext{}); !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("Init error = %v, want deadline exceeded", err)
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
	tests := map[string]struct {
		stateProvider StateProvider
		opts          []Option
		wantInitError bool
	}{
		"init": {
			stateProvider: stateProviderFunc(func(context.Context) ([]byte, string, error) {
				return []byte("state"), "account", nil
			}),
			wantInitError: true,
		},
		"background retry": {
			stateProvider: stateProviderFunc(func() func(context.Context) ([]byte, string, error) {
				var calls atomic.Int32
				return func(context.Context) ([]byte, string, error) {
					if calls.Add(1) == 1 {
						return nil, "", context.DeadlineExceeded
					}
					return []byte("state"), "account", nil
				}
			}()),
			opts: []Option{
				WithInitializationTimeout(5 * time.Millisecond),
				withInitialStateRetryInterval(20 * time.Millisecond),
			},
			wantInitError: true,
		},
	}

	for name, tc := range tests {
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
				tc.stateProvider,
				&tu.MockFlagLogger{},
				"secret",
				nil,
				tc.opts...,
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
				if tc.wantInitError != (err != nil) {
					t.Fatalf("Init error = %v, want error=%v", err, tc.wantInitError)
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
