package confidence

import (
	"context"
	"os"
	"testing"

	"github.com/open-feature/go-sdk/openfeature"
)

// End-to-end tests against the real module CDN, with the e2e credential. They need
// CONFIDENCE_CLIENT_SECRET and CONFIDENCE_CLIENT_ENCRYPTION_KEY (set in CI; locally, see
// openfeature-provider/js/.env.test). Flag logs go to production ingestion.

const (
	e2eIncludedTargetingKey = "user-a"
	e2eExcludedTargetingKey = "user-x"
)

func e2eClient(t *testing.T, mod func(*ProviderConfig)) *openfeature.Client {
	t.Helper()
	secret, key := os.Getenv("CONFIDENCE_CLIENT_SECRET"), os.Getenv("CONFIDENCE_CLIENT_ENCRYPTION_KEY")
	if secret == "" || key == "" {
		if os.Getenv("CI") != "" {
			t.Fatal("CONFIDENCE_CLIENT_SECRET and CONFIDENCE_CLIENT_ENCRYPTION_KEY must be set in CI")
		}
		t.Skip("CONFIDENCE_CLIENT_SECRET and CONFIDENCE_CLIENT_ENCRYPTION_KEY not set")
	}
	cfg := ProviderConfig{ClientSecret: secret, EncryptionKey: key}
	if mod != nil {
		mod(&cfg)
	}
	p, err := NewProvider(context.Background(), cfg)
	if err != nil {
		t.Fatal(err)
	}
	if err := openfeature.SetNamedProviderAndWait(t.Name(), p); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(p.Shutdown)
	return openfeature.NewClient(t.Name())
}

func TestE2E_NonSticky(t *testing.T) {
	client := e2eClient(t, nil)
	ctx := context.Background()
	evalCtx := openfeature.NewEvaluationContext("test-a", map[string]any{"sticky": false})
	d, err := client.BooleanValueDetails(ctx, "web-sdk-e2e-flag.bool", true, evalCtx)
	if err != nil || d.Value {
		t.Errorf("bool: value=%v err=%v", d.Value, err)
	}
	if v, err := client.IntValue(ctx, "web-sdk-e2e-flag.int", 0, evalCtx); err != nil {
		t.Errorf("int: %v %v", v, err)
	}
}

// Without a store, the module decides materialized segment membership itself (from the bloom
// filter compiled into it): both inclusion and exclusion.
func TestE2E_MaterializedSegmentWithoutStore(t *testing.T) {
	client := e2eClient(t, nil)
	for unit, want := range map[string]string{
		e2eIncludedTargetingKey: "flags/custom-targeted-flag/variants/cake-exclamation",
		e2eExcludedTargetingKey: "flags/custom-targeted-flag/variants/default",
	} {
		d, err := client.StringValueDetails(context.Background(), "custom-targeted-flag.message", "client default",
			openfeature.NewTargetlessEvaluationContext(map[string]any{"user_id": unit}))
		if err != nil || d.Variant != want {
			t.Errorf("%s: variant=%q err=%v, want %q", unit, d.Variant, err, want)
		}
	}
}

func remoteStore(c *ProviderConfig) { c.UseRemoteMaterializationStore = true }

// The flag has a running experiment with a sticky assignment for test-3 and paused intake. If this
// breaks, the experiment may have been removed or the materialization cleaned out (see the JS e2e
// test for how to restore it).
func TestE2E_StickyRemoteStore(t *testing.T) {
	client := e2eClient(t, remoteStore)
	d, err := client.FloatValueDetails(context.Background(), "web-sdk-e2e-flag.double", -1,
		openfeature.NewEvaluationContext("test-3", map[string]any{"sticky": true}))
	if err != nil || d.Value != 99.99 || d.Variant != "flags/web-sdk-e2e-flag/variants/sticky" {
		t.Errorf("value=%v variant=%q reason=%v err=%v", d.Value, d.Variant, d.Reason, err)
	}
}

func TestE2E_MaterializedSegmentRemoteStore(t *testing.T) {
	client := e2eClient(t, remoteStore)
	for unit, want := range map[string]string{
		e2eIncludedTargetingKey: "flags/custom-targeted-flag/variants/cake-exclamation",
		e2eExcludedTargetingKey: "flags/custom-targeted-flag/variants/default",
	} {
		d, err := client.StringValueDetails(context.Background(), "custom-targeted-flag.message", "client default",
			openfeature.NewTargetlessEvaluationContext(map[string]any{"user_id": unit}))
		if d.Variant != want {
			t.Errorf("%s: variant=%q (%v), want %q", unit, d.Variant, err, want)
		}
	}
}
