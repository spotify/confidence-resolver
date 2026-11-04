package confidence

import (
	"context"
	"fmt"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"

	cm "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/compiled_module"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
)

// TestRefreshPicksUpReplacementModule serves the tutorial module, lets the
// provider init, then swaps the artifact for the e2e module: the next poll
// must instantiate it and evaluations must reflect the new flags.
func TestRefreshPicksUpReplacementModule(t *testing.T) {
	provider, cdn, key := newCDNProvider(t)
	t.Cleanup(provider.Shutdown)

	if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatalf("Init: %v", err)
	}

	// Replace with a recompiled module for the same credential whose
	// tutorial flag carries a different value.
	v2Module := testutil.LoadClientModuleFixture(t, "tutorial_client_module_v2.wasm")
	cdn.SetArtifact(tutorialSecret, v2Module, "token-2", key)

	// The refreshed value becomes visible once the replacement is live.
	if !awaitCondition(t, 5*time.Second, func() bool {
		detail := provider.StringEvaluation(context.Background(), "tutorial-feature.message", "fallback", tutorialEvalCtx())
		return strings.HasPrefix(detail.Value, "Refreshed rules")
	}) {
		t.Fatal("replacement module was never picked up")
	}
}

// TestRefreshCorruptArtifactKeepsServing serves garbage: the previous module
// must keep serving and the provider must stay READY.
func TestRefreshCorruptArtifactKeepsServing(t *testing.T) {
	provider, cdn, _ := newCDNProvider(t)
	t.Cleanup(provider.Shutdown)

	if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatalf("Init: %v", err)
	}
	before := provider.StringEvaluation(context.Background(), "tutorial-feature.message", "fallback", tutorialEvalCtx())
	if before.Value == "fallback" {
		t.Fatal("precondition: tutorial flag should resolve")
	}

	cdn.SetCorrupt(true)
	// Let a few polls run against the corrupt artifact.
	time.Sleep(200 * time.Millisecond)

	if !provider.ready.Load() {
		t.Error("provider left READY over a corrupt artifact")
	}
	after := provider.StringEvaluation(context.Background(), "tutorial-feature.message", "fallback", tutorialEvalCtx())
	if after.Value != before.Value {
		t.Errorf("serving changed over a corrupt artifact: %q -> %q", before.Value, after.Value)
	}
	cdn.SetCorrupt(false)
}

// TestRefresh404KeepsServing removes the artifact: 404 means "no module
// published", never a reason to stop serving the last good module.
func TestRefresh404KeepsServing(t *testing.T) {
	provider, cdn, _ := newCDNProvider(t)
	t.Cleanup(provider.Shutdown)

	if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatalf("Init: %v", err)
	}

	cdn.RemoveArtifact(tutorialSecret)
	time.Sleep(200 * time.Millisecond)

	if !provider.ready.Load() {
		t.Error("provider left READY on 404")
	}
	detail := provider.StringEvaluation(context.Background(), "tutorial-feature.message", "fallback", tutorialEvalCtx())
	if detail.Value == "fallback" {
		t.Error("last good module stopped serving on 404")
	}
}

// TestWrapperOnlyChangeUpdatesToken serves the same module bytes with a new
// log token: no re-instantiation, and new batches carry the new token.
func TestWrapperOnlyChangeUpdatesToken(t *testing.T) {
	provider, cdn, key := newCDNProvider(t)
	t.Cleanup(provider.Shutdown)

	if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatalf("Init: %v", err)
	}

	hostBefore := provider.currentHostForTest()

	// Same module, new token.
	cdn.SetArtifact(tutorialSecret, testutil.LoadClientModuleFixture(t, tutorialModuleFile), "token-2", key)

	if !awaitCondition(t, 5*time.Second, func() bool {
		token, _ := provider.logToken.Load().(string)
		return token == "token-2"
	}) {
		t.Fatal("wrapper token was not refreshed")
	}

	// Wrapper-only changes must not re-instantiate the module.
	if host := provider.currentHostForTest(); host != hostBefore {
		t.Error("module was re-instantiated for a wrapper-only change")
	}
}

func (p *LocalResolverProvider) currentHostForTest() *cm.Host {
	p.hostMu.RLock()
	defer p.hostMu.RUnlock()
	return p.host
}

// TestConcurrentEvaluationDuringModuleRotation evaluates from many
// goroutines while the artifact rotates underneath: the host swap in
// installModule must wait for in-flight calls. Before the hostMu read lock
// was held for the whole evaluation flow, the teardown raced live calls and
// crashed the process inside wazero (split stack overflow / fault).
func TestConcurrentEvaluationDuringModuleRotation(t *testing.T) {
	provider, cdn, key := newCDNProvider(t)
	t.Cleanup(provider.Shutdown)

	if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatalf("Init: %v", err)
	}

	v1 := testutil.LoadClientModuleFixture(t, tutorialModuleFile)
	v2 := testutil.LoadClientModuleFixture(t, "tutorial_client_module_v2.wasm")

	const evaluators = 10
	const rotations = 4
	var wg sync.WaitGroup
	stop := make(chan struct{})
	var slipped atomic.Int64
	for i := 0; i < evaluators; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for {
				select {
				case <-stop:
					return
				default:
				}
				detail := provider.StringEvaluation(context.Background(), "tutorial-feature.message", "fallback", tutorialEvalCtx())
				v := detail.Value
				// Either module's message is fine mid-rotation; anything
				// else (including the fallback) means a call slipped through
				// a torn state.
				if !strings.HasPrefix(v, "We are very excited") && !strings.HasPrefix(v, "Refreshed rules") {
					slipped.Add(1)
				}
			}
		}()
	}

	// Rotate v1/v2 so each swap re-instantiates the module under load; the
	// last rotation leaves v2 live.
	for i := 0; i < rotations; i++ {
		module := v1
		if i%2 == 1 {
			module = v2
		}
		cdn.SetArtifact(tutorialSecret, module, fmt.Sprintf("token-%d", i+2), key)
		time.Sleep(250 * time.Millisecond)
	}
	close(stop)
	wg.Wait()

	if slipped.Load() != 0 {
		t.Fatalf("%d evaluations slipped through module rotation", slipped.Load())
	}

	if !awaitCondition(t, 5*time.Second, func() bool {
		detail := provider.StringEvaluation(context.Background(), "tutorial-feature.message", "fallback", tutorialEvalCtx())
		return strings.HasPrefix(detail.Value, "Refreshed rules")
	}) {
		t.Fatal("final replacement module was never picked up")
	}
}
