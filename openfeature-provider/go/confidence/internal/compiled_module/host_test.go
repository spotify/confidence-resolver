package compiled_module

import (
	"log/slog"

	"errors"
	"strings"
	"sync"
	"testing"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/flagswasm"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolver"
	resolverv1 "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/resolverinternal"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/structpb"
)

// logCollector captures log bodies and per-batch tokens handed to a host's
// LogSink.
type logCollector struct {
	mu     sync.Mutex
	bodies [][]byte
	tokens []string
}

func (c *logCollector) sink(body []byte, logToken string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.bodies = append(c.bodies, append([]byte(nil), body...))
	c.tokens = append(c.tokens, logToken)
}

func (c *logCollector) all() [][]byte {
	c.mu.Lock()
	defer c.mu.Unlock()
	return append([][]byte(nil), c.bodies...)
}

// flushAndCount flushes the host and returns how many bodies have been
// delivered in total.
func (c *logCollector) flushAndCount(t *testing.T, host *Host) int {
	t.Helper()
	if err := host.FlushLogs(); err != nil {
		t.Fatalf("FlushLogs: %v", err)
	}
	return len(c.all())
}

// newTutorialHost loads the tutorial fixture into a live host.
func newTutorialHost(t *testing.T) (*Host, *logCollector) {
	t.Helper()
	wasm := loadFixture(t, "tutorial_client_module.wasm")
	mod, err := LoadModule(wasm, tutorialSecret)
	if err != nil {
		t.Fatalf("LoadModule: %v", err)
	}
	collector := &logCollector{}
	host, err := NewHost(mod, HostConfig{LogSink: collector.sink})
	if err != nil {
		t.Fatalf("NewHost: %v", err)
	}
	t.Cleanup(host.Close)
	return host, collector
}

func tutorialRequest(t *testing.T, flag string, apply bool) *resolver.ResolveFlagsRequest {
	t.Helper()
	evalCtx, err := structpb.NewStruct(map[string]interface{}{"visitor_id": "tutorial_visitor"})
	if err != nil {
		t.Fatalf("NewStruct: %v", err)
	}
	return &resolver.ResolveFlagsRequest{
		Flags:             []string{flag},
		EvaluationContext: evalCtx,
		ClientSecret:      tutorialSecret,
		Apply:             apply,
	}
}

func TestHostResolveTutorialFlag(t *testing.T) {
	host, _ := newTutorialHost(t)

	result, suspended, err := host.StartResolve(tutorialRequest(t, "flags/tutorial-feature", true))
	if err != nil {
		t.Fatalf("StartResolve: %v", err)
	}
	if suspended != nil {
		t.Fatalf("unexpected suspension: %+v", suspended)
	}
	defer result.Envelope.Close()

	flags := result.Response.GetResolvedFlags()
	if len(flags) != 1 {
		t.Fatalf("resolved %d flags, want 1", len(flags))
	}
	if got := flags[0].GetFlag(); got != "flags/tutorial-feature" {
		t.Errorf("flag = %q", got)
	}
	if got := flags[0].GetVariant(); got != "flags/tutorial-feature/variants/exciting-welcome" {
		t.Errorf("variant = %q", got)
	}
	if flags[0].GetReason() != resolver.ResolveReason_RESOLVE_REASON_MATCH {
		t.Errorf("reason = %s", flags[0].GetReason())
	}
}

func TestHostEvaluate(t *testing.T) {
	host, _ := newTutorialHost(t)

	result, _, err := host.StartResolve(tutorialRequest(t, "flags/tutorial-feature", true))
	if err != nil {
		t.Fatalf("StartResolve: %v", err)
	}
	defer result.Envelope.Close()

	details, err := host.Evaluate(result.Envelope, &flagswasm.EvaluateRequest{
		FlagKey:      "tutorial-feature.message",
		DefaultValue: structpb.NewStringValue("poison-default"),
	})
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if got := details.GetValue().GetStringValue(); !strings.Contains(got, "excited to welcome you") {
		t.Errorf("value = %q", got)
	}
	if details.GetErrorCode() != flagswasm.ErrorCode_ERROR_CODE_UNSPECIFIED {
		t.Errorf("error code = %s", details.GetErrorCode())
	}
	if !details.GetShouldApply() {
		t.Error("should_apply = false, want true for applied resolve")
	}
}

func TestHostEvaluateUnknownKeyReturnsNotFoundAndDefault(t *testing.T) {
	host, _ := newTutorialHost(t)

	result, _, err := host.StartResolve(tutorialRequest(t, "flags/tutorial-feature", true))
	if err != nil {
		t.Fatalf("StartResolve: %v", err)
	}
	defer result.Envelope.Close()

	details, err := host.Evaluate(result.Envelope, &flagswasm.EvaluateRequest{
		FlagKey:      "no-such-flag",
		DefaultValue: structpb.NewBoolValue(true),
	})
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if details.GetErrorCode() != flagswasm.ErrorCode_ERROR_CODE_FLAG_NOT_FOUND {
		t.Errorf("error code = %s, want FLAG_NOT_FOUND", details.GetErrorCode())
	}
	if !details.GetValue().GetBoolValue() {
		t.Errorf("default not echoed back on error path: %v", details.GetValue().AsInterface())
	}
	if details.GetShouldApply() {
		t.Error("should_apply must be false on every error path")
	}
}

func TestHostEvaluateTypeMismatch(t *testing.T) {
	host, _ := newTutorialHost(t)

	result, _, err := host.StartResolve(tutorialRequest(t, "flags/tutorial-feature", true))
	if err != nil {
		t.Fatalf("StartResolve: %v", err)
	}
	defer result.Envelope.Close()

	// The message field is a string; a bool default must come back as a
	// TYPE_MISMATCH error with the default echoed.
	details, err := host.Evaluate(result.Envelope, &flagswasm.EvaluateRequest{
		FlagKey:      "tutorial-feature.message",
		DefaultValue: structpb.NewBoolValue(false),
	})
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if details.GetErrorCode() != flagswasm.ErrorCode_ERROR_CODE_TYPE_MISMATCH {
		t.Errorf("error code = %s, want TYPE_MISMATCH", details.GetErrorCode())
	}
	if details.GetValue().GetBoolValue() != false {
		t.Errorf("default not echoed: %v", details.GetValue().AsInterface())
	}
}

func TestHostResolveUnknownSecret(t *testing.T) {
	host, _ := newTutorialHost(t)

	evalCtx, _ := structpb.NewStruct(map[string]interface{}{"visitor_id": "x"})
	result, _, err := host.StartResolve(&resolver.ResolveFlagsRequest{
		Flags:             []string{"flags/tutorial-feature"},
		EvaluationContext: evalCtx,
		ClientSecret:      "wrong-secret-entirely",
		Apply:             true,
	})
	if err != nil {
		// A newer producer rejects unknown credentials with a
		// ResolveRejectedError (gRPC UNAUTHENTICATED); that path is also fine.
		var rejected *ResolveRejectedError
		if ok := errors.As(err, &rejected); ok && rejected.IsUnauthenticated() {
			return
		}
		t.Fatalf("unexpected error: %v", err)
	}
	defer result.Envelope.Close()

	// The current module answers an unknown secret with an empty resolve; the
	// evaluation step then reports the flag as not found.
	if flags := result.Response.GetResolvedFlags(); len(flags) != 0 {
		t.Fatalf("unknown secret resolved %d flags, want 0", len(flags))
	}
	details, err := host.Evaluate(result.Envelope, &flagswasm.EvaluateRequest{
		FlagKey:      "tutorial-feature.message",
		DefaultValue: structpb.NewStringValue("fallback"),
	})
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if details.GetErrorCode() != flagswasm.ErrorCode_ERROR_CODE_FLAG_NOT_FOUND {
		t.Errorf("error code = %s, want FLAG_NOT_FOUND", details.GetErrorCode())
	}
	if got := details.GetValue().GetStringValue(); got != "fallback" {
		t.Errorf("default not echoed: %q", got)
	}
}

func TestHostLogsDeliveredAndAcked(t *testing.T) {
	host, collector := newTutorialHost(t)

	if _, _, err := host.StartResolve(tutorialRequest(t, "flags/tutorial-feature", true)); err != nil {
		t.Fatalf("StartResolve: %v", err)
	}

	if collector.flushAndCount(t, host) == 0 {
		t.Fatal("no log bodies delivered after resolve+flush")
	}
	var sawAssignment bool
	for _, body := range collector.all() {
		logReq := &resolverv1.WriteFlagLogsRequest{}
		if err := proto.Unmarshal(body, logReq); err != nil {
			t.Fatalf("log body does not decode as WriteFlagLogsRequest: %v", err)
		}
		if len(logReq.GetFlagAssigned()) > 0 || len(logReq.GetFlagResolveInfo()) > 0 {
			sawAssignment = true
		}
	}
	if !sawAssignment {
		t.Error("no applied flag present in any delivered log batch")
	}
}

func TestHostDeferredApply(t *testing.T) {
	host, _ := newTutorialHost(t)

	// Resolve with apply=false: the response carries a resolve token.
	result, _, err := host.StartResolve(tutorialRequest(t, "flags/tutorial-feature", false))
	if err != nil {
		t.Fatalf("StartResolve: %v", err)
	}
	token := result.Response.GetResolveToken()
	if len(token) == 0 {
		t.Fatal("apply=false resolve did not return a token")
	}

	status, err := host.ApplyFlags(&resolver.ApplyFlagsRequest{
		ClientSecret: tutorialSecret,
		ResolveToken: token,
		Flags:        []*resolver.AppliedFlag{{Flag: "flags/tutorial-feature"}},
	})
	if err != nil {
		t.Fatalf("ApplyFlags: %v", err)
	}
	if status != ApplyOK {
		t.Fatalf("ApplyFlags status = %s", status)
	}
	result.Envelope.Close()
}

// TestHostConcurrentEntryServesEachCallOnItsOwnInstance runs many
// goroutines through full resolve+evaluate flows at once: the contract's
// threading model is one instance per concurrent call over the one shared
// memory, so the pool must serve them in parallel without corrupting the
// flag data or the shared heap.
func TestHostConcurrentEntryServesEachCallOnItsOwnInstance(t *testing.T) {
	// A small explicit cap makes the pool's growth deterministic.
	wasm := loadFixture(t, "tutorial_client_module.wasm")
	mod, err := LoadModule(wasm, tutorialSecret)
	if err != nil {
		t.Fatalf("LoadModule: %v", err)
	}
	collector := &logCollector{}
	host, err := NewHost(mod, HostConfig{LogSink: collector.sink, InstanceCap: 4})
	if err != nil {
		t.Fatalf("NewHost: %v", err)
	}
	t.Cleanup(host.Close)

	const goroutines = 16
	const iterations = 25
	var wg sync.WaitGroup
	for g := 0; g < goroutines; g++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for i := 0; i < iterations; i++ {
				result, _, err := host.StartResolve(tutorialRequest(t, "flags/tutorial-feature", true))
				if err != nil {
					t.Errorf("StartResolve: %v", err)
					return
				}
				details, err := host.Evaluate(result.Envelope, &flagswasm.EvaluateRequest{
					FlagKey:      "tutorial-feature.message",
					DefaultValue: structpb.NewStringValue("fallback"),
				})
				if err != nil {
					t.Errorf("Evaluate: %v", err)
					result.Envelope.Close()
					return
				}
				if got := details.GetValue().GetStringValue(); !strings.HasPrefix(got, "We are very excited") {
					t.Errorf("value corrupted under concurrency: %q", got)
					result.Envelope.Close()
					return
				}
				result.Envelope.Close()
			}
		}()
	}
	wg.Wait()

	// The pool must have grown past the warm instance to serve the
	// concurrency (and never past its cap).
	host.mu.Lock()
	live := host.live
	host.mu.Unlock()
	if live < 2 || live > 4 {
		t.Fatalf("instance pool live = %d, want between 2 and 4", live)
	}
}

// TestDeliverLogsCarriesModuleLogToken: the write_logs payload may echo a
// compile-time log token alongside the parts; the sink must receive both,
// with an empty token when the module was compiled without one.
func TestDeliverLogsCarriesModuleLogToken(t *testing.T) {
	host, collector := newTutorialHost(t)

	inst, err := host.acquire()
	if err != nil {
		t.Fatalf("acquire: %v", err)
	}
	defer host.release(inst)

	for _, tc := range []struct {
		name  string
		token string
	}{
		{name: "with-token", token: "compile-time-token"},
		{name: "without-token", token: ""},
	} {
		partSlice, err := host.writeSlice(inst, []byte("hello-"+tc.name))
		if err != nil {
			t.Fatalf("%s: part: %v", tc.name, err)
		}
		payload, err := host.writeSlice(inst, mustMarshal(&flagswasm.WriteLogsPayload{
			Parts:    []uint64{partSlice},
			LogToken: tc.token,
		}))
		if err != nil {
			t.Fatalf("%s: payload: %v", tc.name, err)
		}

		before := len(collector.bodies)
		host.deliverLogs(inst.mod, payload, slog.Default())
		if got := len(collector.bodies) - before; got != 1 {
			t.Fatalf("%s: sink called %d times, want 1", tc.name, got)
		}
		if got := collector.bodies[before]; string(got) != "hello-"+tc.name {
			t.Errorf("%s: body = %q", tc.name, got)
		}
		if got := collector.tokens[before]; got != tc.token {
			t.Errorf("%s: token = %q, want %q", tc.name, got, tc.token)
		}
	}
}
