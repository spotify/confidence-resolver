package confidence

import (
	"context"
	"errors"
	"testing"

	"github.com/open-feature/go-sdk/openfeature"
)

// Behavioral parity tests recovered from the state-based provider's suite:
// the compiled-module rewrite must preserve the provider's observable
// behavior for equivalent inputs. Each test restates an assertion the old
// suite made, adapted to the module fixtures.

// TestParityPathNotFoundInFlag: evaluating a flag that exists but asking
// for a value path that does not must fail with the default echoed.
//
// Intentional delta from the state-based provider, documented here: the
// old Go-side path walking returned FLAG_NOT_FOUND ("path 'x' not found in
// flag 'y'"); evaluation now belongs to the module, whose contract (shared
// by every provider on this architecture) classifies a missing path as a
// type mismatch — the flag resolved, the requested path's value is absent,
// so it cannot match the default's type. Unknown FLAGS still yield
// FLAG_NOT_FOUND.
func TestParityPathNotFoundInFlag(t *testing.T) {
	provider, _ := newTestProvider(t)

	for _, tc := range []struct {
		key string
	}{
		{key: "tutorial-feature.nonexistent"},
		{key: "tutorial-feature.message.deeply.nested"},
	} {
		detail := provider.StringEvaluation(context.Background(), tc.key, "fallback", tutorialEvalCtx())
		if detail.Value != "fallback" {
			t.Errorf("%s: value = %q, want the echoed default", tc.key, detail.Value)
		}
		if detail.Reason != openfeature.ErrorReason {
			t.Errorf("%s: reason = %q, want ErrorReason", tc.key, detail.Reason)
		}
		if got := errorCodeOf(detail.ResolutionError); got != openfeature.TypeMismatchCode {
			t.Errorf("%s: error code = %q, want TypeMismatchCode", tc.key, got)
		}
	}
}

// TestParityMissingMaterializationsMessage: a resolve that needs
// materializations with no store configured surfaces the documented error
// message naming the flag, echoes the default, and keeps the provider
// serving.
func TestParityMissingMaterializationsMessage(t *testing.T) {
	provider := newE2EProvider(t, nil)

	detail := provider.StringEvaluation(context.Background(), "custom-targeted-flag.message", "fallback", cakeEvalCtx())
	if detail.Value != "fallback" {
		t.Errorf("value = %q, want the echoed default", detail.Value)
	}
	if detail.Reason != openfeature.ErrorReason {
		t.Errorf("reason = %q, want ErrorReason", detail.Reason)
	}
	// The old suite pinned this message via the SDK error string
	// ("error code: GENERAL: <message>"); ResolutionError.Error() carries
	// the same content with its code prefix.
	want := "GENERAL: flag 'custom-targeted-flag' requires materializations; configure a materialization store"
	if got := detail.ResolutionError.Error(); got != want {
		t.Errorf("error = %q, want %q", got, want)
	}
}

// failingReadStore wraps a MaterializationStore, failing every read.
type failingReadStore struct {
	inner MaterializationStore
}

func (s *failingReadStore) Read(ctx context.Context, ops []ReadOp) ([]ReadResult, error) {
	return nil, errors.New("store read failed")
}

func (s *failingReadStore) Write(ctx context.Context, ops []WriteOp) error {
	return s.inner.Write(ctx, ops)
}

// TestParityStoreReadErrorFailsResolve: an unreadable materialization store
// fails the resolve that needed it (the error surfaces instead of silently
// resolving without the stored assignment), with the default echoed.
func TestParityStoreReadErrorFailsResolve(t *testing.T) {
	provider := newE2EProvider(t, &failingReadStore{
		inner: newInMemoryMaterializationStore(testLogger(t)),
	})

	detail := provider.StringEvaluation(context.Background(), "custom-targeted-flag.message", "fallback", cakeEvalCtx())
	if detail.Value != "fallback" {
		t.Errorf("value = %q, want the echoed default", detail.Value)
	}
	if detail.Reason != openfeature.ErrorReason {
		t.Errorf("reason = %q, want ErrorReason", detail.Reason)
	}
	if errorCodeOf(detail.ResolutionError) == "" {
		t.Fatal("expected the store read failure to surface")
	}
}
