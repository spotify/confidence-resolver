package engine

import (
	"context"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"google.golang.org/protobuf/types/known/structpb"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/s2wpb/resolverpb"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/s2wpb/wasmpb"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
)

const secret = testutil.Secret

func fixture(t *testing.T) []byte { return testutil.Module(t, "basic.wasm") }

func newEngine(t *testing.T, b *testutil.Backend, mod func(*Config)) *Engine {
	cfg := Config{
		ClientSecret: secret, EncryptionKey: b.Key, CDNBaseURL: b.CDN.URL, IngestBaseURL: b.Ingest.URL, ResolverBaseURL: b.Resolver.URL,
		Instances: 4, PollInterval: 10 * time.Millisecond, FlushInterval: time.Hour, RetryInterval: 5 * time.Millisecond,
		Logger: slog.New(slog.NewTextHandler(io.Discard, nil)),
	}
	if mod != nil {
		mod(&cfg)
	}
	e, err := New(context.Background(), cfg)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = e.Close(context.Background()) })
	return e
}

func tokens(b *testutil.Backend) map[string]int {
	m := map[string]int{}
	for _, batch := range b.Batches() {
		m[batch.Token]++
	}
	return m
}

type noStore struct{}

func (noStore) Read(context.Context, []*wasmpb.Materialization_Record) ([]*wasmpb.Materialization_Record, error) {
	return nil, nil
}
func (noStore) Write(context.Context, []*wasmpb.Materialization_Record) error { return nil }

var evalEnabled = []*wasmpb.EvaluateRequest{{FlagKey: "plain.enabled", DefaultValue: structpb.NewBoolValue(false)}}

func evaluate(e *Engine, unit string) error {
	ctx, _ := structpb.NewStruct(map[string]any{"targeting_key": unit})
	d, err := e.Evaluate(context.Background(), &resolverpb.ResolveFlagsRequest{
		Flags: []string{"flags/plain"}, EvaluationContext: ctx, ClientSecret: secret, Apply: true,
	}, evalEnabled, noStore{})
	if err != nil {
		return err
	}
	if !d[0].GetValue().GetBoolValue() {
		return fmt.Errorf("unexpected value %v", d[0].GetValue())
	}
	return nil
}

func eventually(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for !cond() {
		if time.Now().After(deadline) {
			t.Fatalf("timed out waiting for %s", what)
		}
		time.Sleep(5 * time.Millisecond)
	}
}

func TestStartAndEvaluate(t *testing.T) {
	b := testutil.NewBackend(t)
	b.Publish(fixture(t), "tok-1", `"v1"`)
	e := newEngine(t, b, nil)
	if err := evaluate(e, "u"); !errors.Is(err, ErrNotReady) {
		t.Errorf("before Start: %v", err)
	}
	if err := e.Start(context.Background(), time.Second); err != nil {
		t.Fatal(err)
	}
	if err := evaluate(e, "u"); err != nil {
		t.Fatal(err)
	}
	if err := e.Close(context.Background()); err != nil {
		t.Fatal(err)
	}
	if tokens(b)["tok-1"] == 0 {
		t.Errorf("close didn't ship logs with the module's token: %v", tokens(b))
	}
	if err := evaluate(e, "u"); !errors.Is(err, ErrClosed) {
		t.Errorf("after Close: %v", err)
	}
}

func TestSwapUnderLoad(t *testing.T) {
	b := testutil.NewBackend(t)
	wasm := fixture(t)
	b.Publish(wasm, "tok-0", `"v0"`)
	e := newEngine(t, b, nil)
	if err := e.Start(context.Background(), time.Second); err != nil {
		t.Fatal(err)
	}
	var stop atomic.Bool
	var failures, done atomic.Int64
	var wg sync.WaitGroup
	for g := range 8 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for i := 0; !stop.Load(); i++ {
				if err := evaluate(e, fmt.Sprintf("u-%d-%d", g, i)); err != nil {
					failures.Add(1)
					t.Log(err)
				}
				done.Add(1)
			}
		}()
	}
	const swaps = 5
	for i := 1; i <= swaps; i++ {
		before := e.current.Load()
		b.Publish(wasm, fmt.Sprintf("tok-%d", i), fmt.Sprintf(`"v%d"`, i))
		eventually(t, "swap", func() bool { return e.current.Load() != before })
	}
	stop.Store(true)
	wg.Wait()
	if err := e.Close(context.Background()); err != nil {
		t.Fatal(err)
	}
	if failures.Load() != 0 {
		t.Fatalf("%d of %d evaluations failed across %d swaps", failures.Load(), done.Load(), swaps)
	}
	t.Logf("%d evaluations across %d swaps", done.Load(), swaps)
	got := tokens(b)
	for i := 0; i <= swaps; i++ {
		if got[fmt.Sprintf("tok-%d", i)] == 0 {
			t.Errorf("no logs shipped with generation %d's token: %v", i, got)
		}
	}
}

func TestBrokenModuleKeepsCurrentAndIsRefetched(t *testing.T) {
	b := testutil.NewBackend(t)
	b.Publish(fixture(t), "tok-good", `"good"`)
	e := newEngine(t, b, nil)
	if err := e.Start(context.Background(), time.Second); err != nil {
		t.Fatal(err)
	}
	good := e.current.Load()
	b.Publish([]byte("not wasm"), "tok-bad", `"bad"`)
	eventually(t, "refetches of the broken module", func() bool {
		n := 0
		for _, inm := range b.IfNoneMatch() {
			if inm == `"good"` {
				n++
			}
		}
		return n >= 3 // every poll is still conditional on the good ETag
	})
	if e.current.Load() != good {
		t.Error("broken module replaced the working one")
	}
	if err := evaluate(e, "u"); err != nil {
		t.Errorf("evaluate after broken publish: %v", err)
	}
}

func TestStartup(t *testing.T) {
	t.Run("404 then published", func(t *testing.T) {
		b := testutil.NewBackend(t)
		b.SetStatus(404)
		e := newEngine(t, b, nil)
		go func() {
			time.Sleep(30 * time.Millisecond)
			b.Publish(fixture(t), "tok", `"v"`)
		}()
		if err := e.Start(context.Background(), 5*time.Second); err != nil {
			t.Fatal(err)
		}
	})
	t.Run("forbidden is terminal", func(t *testing.T) {
		b := testutil.NewBackend(t)
		b.SetStatus(403)
		var term *TerminalError
		if err := newEngine(t, b, nil).Start(context.Background(), time.Second); !errors.As(err, &term) {
			t.Fatalf("err = %v", err)
		}
	})
	t.Run("invalid first module is terminal", func(t *testing.T) {
		b := testutil.NewBackend(t)
		b.Publish([]byte("not wasm"), "tok", `"v"`)
		var term *TerminalError
		if err := newEngine(t, b, nil).Start(context.Background(), time.Second); !errors.As(err, &term) {
			t.Fatalf("err = %v", err)
		}
	})
	t.Run("timeout, then ready", func(t *testing.T) {
		b := testutil.NewBackend(t)
		b.SetStatus(503)
		ready := make(chan struct{})
		e := newEngine(t, b, func(c *Config) { c.OnReady = func() { close(ready) } })
		var term *TerminalError
		if err := e.Start(context.Background(), 30*time.Millisecond); err == nil || errors.As(err, &term) {
			t.Fatalf("err = %v, want a non-terminal timeout", err)
		}
		b.Publish(fixture(t), "tok", `"v"`)
		select {
		case <-ready:
		case <-time.After(5 * time.Second):
			t.Fatal("OnReady not called")
		}
		if err := evaluate(e, "u"); err != nil {
			t.Fatal(err)
		}
	})
}

// blockingStore's reads wait for release.
type blockingStore struct {
	noStore
	entered, release chan struct{}
}

func (s blockingStore) Read(ctx context.Context, req []*wasmpb.Materialization_Record) ([]*wasmpb.Materialization_Record, error) {
	close(s.entered)
	<-s.release
	return nil, nil
}

// Close honors its deadline while an evaluation is in a materialization read, and the evaluation's
// generation is cleaned up, its logs shipped, once the read returns.
func TestCloseDeadlineWithReadInFlight(t *testing.T) {
	b := testutil.NewBackend(t)
	b.Publish(fixture(t), "tok-1", `"v1"`)
	e := newEngine(t, b, nil)
	if err := e.Start(context.Background(), time.Second); err != nil {
		t.Fatal(err)
	}
	store := blockingStore{entered: make(chan struct{}), release: make(chan struct{})}
	done := make(chan error, 1)
	go func() {
		ctx, _ := structpb.NewStruct(map[string]any{"targeting_key": "u"})
		_, err := e.Evaluate(context.Background(), &resolverpb.ResolveFlagsRequest{
			Flags: []string{"flags/sticky"}, EvaluationContext: ctx, ClientSecret: secret, Apply: true,
		}, []*wasmpb.EvaluateRequest{{FlagKey: "sticky.value", DefaultValue: structpb.NewNumberValue(-1)}}, store)
		done <- err
	}()
	<-store.entered

	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Millisecond)
	defer cancel()
	start := time.Now()
	if err := e.Close(ctx); !errors.Is(err, context.DeadlineExceeded) {
		t.Errorf("Close = %v, want DeadlineExceeded", err)
	}
	if d := time.Since(start); d > time.Second {
		t.Errorf("Close took %v with a 20ms deadline", d)
	}
	if err := evaluate(e, "u"); !errors.Is(err, ErrClosed) {
		t.Errorf("after Close: %v", err)
	}

	close(store.release)
	if err := <-done; err != nil {
		t.Errorf("in-flight evaluation: %v", err)
	}
	eventually(t, "the retired generation's logs", func() bool { return tokens(b)["tok-1"] > 0 })
}
