package confidence

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
	"github.com/open-feature/go-sdk/openfeature/isolated"
	lr "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/local_resolver"
	admin "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/admin"
	resolverinternal "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	tu "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
	"google.golang.org/protobuf/proto"
)

type lifecycleTransport func(*http.Request) (*http.Response, error)

type lifecycleFlagLogger struct{}

func (lifecycleFlagLogger) Write(*resolverinternal.WriteFlagLogsRequest) {}
func (lifecycleFlagLogger) Shutdown()                                    {}

func (f lifecycleTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

func stateResponse(status int, body []byte, etag string) *http.Response {
	return &http.Response{StatusCode: status, Body: io.NopCloser(bytes.NewReader(body)), Header: http.Header{"Etag": {etag}}}
}

func encryptedClientState(t *testing.T, state []byte, account string) []byte {
	t.Helper()
	b, err := proto.Marshal(&admin.ClientResolverState{State: state, Account: account})
	if err != nil {
		t.Fatal(err)
	}
	return encryptTestState(t, b)
}

func TestStateFetchFailureClassification(t *testing.T) {
	for _, status := range []int{400, 401, 403, 404, 408, 422, 429, 500, 503} {
		t.Run(fmt.Sprint(status), func(t *testing.T) {
			fetcher, err := NewFlagsAdminStateFetcherWithTransport("secret", testEncryptionKey, slog.Default(), lifecycleTransport(func(*http.Request) (*http.Response, error) {
				return stateResponse(status, nil, ""), nil
			}))
			if err != nil {
				t.Fatal(err)
			}
			_, _, err = fetcher.Provide(context.Background())
			terminal := status < 500 && status != 404 && status != 408 && status != 429
			if err == nil || isTerminalStateError(err) != terminal {
				t.Fatalf("status %d: error=%v, terminal=%v", status, err, isTerminalStateError(err))
			}
		})
	}
}

func TestOpenFeatureTerminalStartup(t *testing.T) {
	for name, response := range map[string]*http.Response{
		"unauthorized":  stateResponse(401, nil, ""),
		"decryption":    stateResponse(200, []byte("invalid ciphertext"), ""),
		"decode":        stateResponse(200, encryptTestState(t, []byte{0xff}), ""),
		"empty account": stateResponse(200, encryptedClientState(t, nil, ""), ""),
	} {
		t.Run(name, func(t *testing.T) {
			var calls atomic.Int32
			fetcher, err := NewFlagsAdminStateFetcherWithTransport("secret", testEncryptionKey, slog.Default(), lifecycleTransport(func(*http.Request) (*http.Response, error) {
				calls.Add(1)
				return response, nil
			}))
			if err != nil {
				t.Fatal(err)
			}
			provider := newStartupTestProvider(fetcher, &mockResolverAPIForInit{}, WithInitializationTimeout(time.Second), withInitialStateRetryInterval(time.Millisecond))
			api := isolated.NewAPI()
			t.Cleanup(func() { _ = api.Shutdown(context.Background()) })
			started := time.Now()
			err = api.SetProviderAndWait(context.Background(), provider)
			var initErr *openfeature.ProviderInitError
			if !errors.As(err, &initErr) || initErr.ErrorCode != openfeature.ProviderFatalCode || initErr.Message == "" {
				t.Fatalf("expected useful fatal error, got %v", err)
			}
			if time.Since(started) > 500*time.Millisecond {
				t.Fatal("terminal initialization waited for startup budget")
			}
			client := api.NewClient()
			waitFor(t, time.Second, func() bool { return client.State() == openfeature.FatalState })
			value, err := client.BooleanValue(context.Background(), "flag", true, openfeature.EvaluationContext{})
			if !value || err == nil {
				t.Fatalf("fatal evaluation = %v, %v", value, err)
			}
			time.Sleep(15 * time.Millisecond)
			if calls.Load() != 1 {
				t.Fatalf("terminal state retried %d times", calls.Load())
			}
		})
	}
}

func TestOpenFeatureRejectedInitialResolverState(t *testing.T) {
	var calls atomic.Int32
	provider := newLocalResolverProvider(newLocalResolverSupplier(1, false, nil), stateProviderFunc(func(context.Context) ([]byte, string, error) {
		calls.Add(1)
		return []byte{0xff}, "account", nil
	}), lifecycleFlagLogger{}, "secret", nil, WithInitializationTimeout(time.Second))
	api := isolated.NewAPI()
	defer api.Shutdown(context.Background())
	err := api.SetProviderAndWait(context.Background(), provider)
	var initErr *openfeature.ProviderInitError
	if !errors.As(err, &initErr) || initErr.ErrorCode != openfeature.ProviderFatalCode || !strings.Contains(initErr.Message, "decode") {
		t.Fatalf("rejected state was not fatal: %v", err)
	}
	if calls.Load() != 1 {
		t.Fatal("rejected state was retried")
	}
}

func TestOpenFeatureCachedStateSurvivesRefreshFailures(t *testing.T) {
	good := encryptedClientState(t, tu.LoadTestResolverState(t), tu.LoadTestAccountID(t))
	bad := encryptedClientState(t, []byte{0xff}, tu.LoadTestAccountID(t))
	var phase atomic.Int32
	var requests [6]atomic.Int32
	blocked := make(chan struct{}, 1)
	canceled := make(chan struct{}, 1)
	fetcher, err := NewFlagsAdminStateFetcherWithTransport(tu.TestClientSecret, testEncryptionKey, slog.Default(), lifecycleTransport(func(req *http.Request) (*http.Response, error) {
		current := phase.Load()
		requests[current].Add(1)
		switch current {
		case 1:
			if req.Header.Get("If-None-Match") == "bad" {
				return stateResponse(304, nil, ""), nil
			}
			return stateResponse(200, bad, "bad"), nil
		case 3:
			return stateResponse(304, nil, ""), nil
		case 4:
			blocked <- struct{}{}
			<-req.Context().Done()
			canceled <- struct{}{}
			return nil, req.Context().Err()
		case 5:
			return stateResponse(403, nil, ""), nil
		}
		return stateResponse(200, good, "good"), nil
	}))
	if err != nil {
		t.Fatal(err)
	}
	provider := newLocalResolverProvider(func(ctx context.Context, sink lr.LogSink) lr.LocalResolver {
		return lr.NewLocalResolverWithPoolSize(ctx, sink, 1)
	}, fetcher, lifecycleFlagLogger{}, tu.TestClientSecret, nil, WithStatePollInterval(5*time.Millisecond))
	api := isolated.NewAPI()
	defer api.Shutdown(context.Background())
	if err := api.SetProviderAndWait(context.Background(), provider); err != nil {
		t.Fatal(err)
	}
	client := api.NewClient()
	ctx := context.Background()
	evalCtx := openfeature.NewTargetlessEvaluationContext(map[string]interface{}{"visitor_id": "tutorial_visitor"})
	assertCached := func() {
		t.Helper()
		if client.State() != openfeature.ReadyState {
			t.Fatalf("provider state = %s, want READY", client.State())
		}
		value, err := client.StringValue(ctx, "tutorial-feature.message", "default", evalCtx)
		if err != nil || value == "default" {
			t.Fatalf("cached evaluation = %q, %v", value, err)
		}
	}
	refreshPhase := func(next int32) {
		t.Helper()
		before := requests[next].Load()
		phase.Store(next)
		// Polling is sequential: the third request proves two previous refreshes
		// completed, including a 304 after the rejected payload in phase 1.
		waitFor(t, time.Second, func() bool { return requests[next].Load() >= before+3 })
	}
	assertCached()
	for _, failedPhase := range []int32{1, 5} {
		refreshPhase(failedPhase)
		assertCached()
		refreshPhase(2)
		assertCached()
	}
	refreshPhase(3)
	assertCached()
	phase.Store(4)
	select {
	case <-blocked:
	case <-time.After(time.Second):
		t.Fatal("request did not block")
	}
	assertCached()
	if err := api.Shutdown(ctx); err != nil {
		t.Fatal(err)
	}
	select {
	case <-canceled:
	case <-time.After(time.Second):
		t.Fatal("shutdown did not cancel blocked refresh")
	}
}

func TestOpenFeatureInitial304Recovery(t *testing.T) {
	var recoverNow atomic.Bool
	good := encryptedClientState(t, []byte("state"), "account")
	fetcher, err := NewFlagsAdminStateFetcherWithTransport("secret", testEncryptionKey, slog.Default(), lifecycleTransport(func(*http.Request) (*http.Response, error) {
		if recoverNow.Load() {
			return stateResponse(200, good, "good"), nil
		}
		return stateResponse(304, nil, ""), nil
	}))
	if err != nil {
		t.Fatal(err)
	}
	provider := newStartupTestProvider(fetcher, &mockResolverAPIForInit{}, WithInitializationTimeout(25*time.Millisecond), withInitialStateRetryInterval(5*time.Millisecond))
	api := isolated.NewAPI()
	defer api.Shutdown(context.Background())
	if err := api.SetProviderAndWait(context.Background(), provider); !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("initial 304 should remain recoverable, got %v", err)
	}
	client := api.NewClient()
	waitFor(t, time.Second, func() bool { return client.State() == openfeature.ErrorState })
	if provider.ready.Load() {
		t.Fatal("initial 304 made provider ready")
	}
	recoverNow.Store(true)
	waitFor(t, time.Second, func() bool { return client.State() == openfeature.ReadyState })
}

func TestOpenFeatureTerminalFailureDuringStartupRecovery(t *testing.T) {
	var failTerminal atomic.Bool
	var calls atomic.Int32
	provider := newStartupTestProvider(stateProviderFunc(func(context.Context) ([]byte, string, error) {
		calls.Add(1)
		if failTerminal.Load() {
			return nil, "", &terminalStateError{errors.New("credentials rejected")}
		}
		return nil, "", context.DeadlineExceeded
	}), &mockResolverAPIForInit{}, WithInitializationTimeout(25*time.Millisecond), withInitialStateRetryInterval(5*time.Millisecond))
	api := isolated.NewAPI()
	defer api.Shutdown(context.Background())
	if err := api.SetProviderAndWait(context.Background(), provider); !errors.Is(err, context.DeadlineExceeded) {
		t.Fatal(err)
	}
	client := api.NewClient()
	failTerminal.Store(true)
	waitFor(t, time.Second, func() bool { return client.State() == openfeature.FatalState })
	before := calls.Load()
	time.Sleep(20 * time.Millisecond)
	if calls.Load() != before {
		t.Fatal("terminal background startup error was retried")
	}
}
