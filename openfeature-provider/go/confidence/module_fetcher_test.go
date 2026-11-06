package confidence

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"testing"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/testutil"
)

// tutorialSecret matches the credential baked into the tutorial module
// fixture; it is not a live secret.
const tutorialSecret = "mkjJruAATQWjeY7foFIWfVAcBWnci2YF"

func TestModuleObjectPathUsesExactSecretBytes(t *testing.T) {
	// The path is derived from the exact UTF-8 bytes: no trimming,
	// case-folding or normalisation, including for secrets with trailing
	// whitespace and mixed case.
	cases := []struct {
		name   string
		secret string
	}{
		{"plain", tutorialSecret},
		{"trailing space", tutorialSecret + " "},
		{"mixed case", "MiXeD-CaSe-Secret"},
		{"unicode", "sêcret-ünicode"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			want := sha256.Sum256([]byte(tc.secret))
			got := moduleObjectPath(tc.secret)
			wantPath := "modules/" + hex.EncodeToString(want[:]) + ".wasm.enc"
			if got != wantPath {
				t.Errorf("path = %q, want %q", got, wantPath)
			}
		})
	}
}

func TestModuleFetcherFetchesDecryptsAndParses(t *testing.T) {
	cdn := testutil.NewMockModuleCDN(t)
	key := testutil.GenerateEncryptionKey()
	module := testutil.LoadClientModuleFixture(t, "tutorial_client_module.wasm")
	cdn.SetArtifact(tutorialSecret, module, "test-log-token", key)

	fetcher, err := NewModuleFetcher(tutorialSecret, key, cdn.URL(), nil, cdn.HTTPClient())
	if err != nil {
		t.Fatalf("NewModuleFetcher: %v", err)
	}

	fetched, err := fetcher.Fetch(context.Background())
	if err != nil {
		t.Fatalf("Fetch: %v", err)
	}
	if len(fetched.Module) == 0 {
		t.Fatal("no module bytes")
	}
	if fetched.LogToken != "test-log-token" {
		t.Errorf("log token = %q", fetched.LogToken)
	}
	if fetched.ETag == "" {
		t.Error("no ETag captured")
	}
}

func TestModuleFetcherSendsIfNoneMatchOnSecondPoll(t *testing.T) {
	cdn := testutil.NewMockModuleCDN(t)
	key := testutil.GenerateEncryptionKey()
	module := testutil.LoadClientModuleFixture(t, "tutorial_client_module.wasm")
	cdn.SetArtifact(tutorialSecret, module, "tok", key)

	fetcher, _ := NewModuleFetcher(tutorialSecret, key, cdn.URL(), nil, cdn.HTTPClient())
	if _, err := fetcher.Fetch(context.Background()); err != nil {
		t.Fatalf("first fetch: %v", err)
	}

	// The second poll sends If-None-Match and the CDN answers 304.
	_, err := fetcher.Fetch(context.Background())
	if err == nil || !isModuleUnchanged(err) {
		t.Fatalf("second fetch err = %v, want moduleUnchangedError", err)
	}

	reqs := cdn.Requests()
	if len(reqs) != 2 {
		t.Fatalf("served %d requests, want 2", len(reqs))
	}
	if reqs[1].INMValue == "" {
		t.Error("second poll did not send If-None-Match")
	}
}

func TestModuleFetcher404IsNotPublished(t *testing.T) {
	cdn := testutil.NewMockModuleCDN(t)
	key := testutil.GenerateEncryptionKey()

	fetcher, _ := NewModuleFetcher("no-such-credential", key, cdn.URL(), nil, cdn.HTTPClient())
	_, err := fetcher.Fetch(context.Background())
	if err == nil {
		t.Fatal("expected error for missing artifact")
	}
	if !isModuleNotPublished(err) {
		t.Errorf("expected moduleNotPublishedError, got %v", err)
	}
}

func TestModuleFetcherDecryptFailureKeepsRetryable(t *testing.T) {
	cdn := testutil.NewMockModuleCDN(t)
	key := testutil.GenerateEncryptionKey()
	module := testutil.LoadClientModuleFixture(t, "tutorial_client_module.wasm")
	// Artifact encrypted with a DIFFERENT key than the fetcher holds: the
	// expected symptom of an encryption-key rotation not yet picked up.
	otherKey := testutil.GenerateEncryptionKey()
	cdn.SetArtifact(tutorialSecret, module, "tok", otherKey)

	fetcher, _ := NewModuleFetcher(tutorialSecret, key, cdn.URL(), nil, cdn.HTTPClient())
	_, err := fetcher.Fetch(context.Background())
	if err == nil {
		t.Fatal("expected decrypt failure")
	}
	if isModuleNotPublished(err) {
		t.Error("decrypt failure must not be classified as not-published")
	}
}

func TestModuleFetcherCorruptArtifactKeepsRetryable(t *testing.T) {
	cdn := testutil.NewMockModuleCDN(t)
	key := testutil.GenerateEncryptionKey()
	module := testutil.LoadClientModuleFixture(t, "tutorial_client_module.wasm")
	cdn.SetArtifact(tutorialSecret, module, "tok", key)
	cdn.SetCorrupt(true)

	fetcher, _ := NewModuleFetcher(tutorialSecret, key, cdn.URL(), nil, cdn.HTTPClient())
	_, err := fetcher.Fetch(context.Background())
	if err == nil {
		t.Fatal("expected fetch failure on corrupt artifact")
	}
}

func TestModuleFetcherRejectsInvalidKey(t *testing.T) {
	cases := []string{"", "not-hex", "tooshort", "0123456789abcdef0123456789abcdef"}
	for _, key := range cases {
		if _, err := NewModuleFetcher(tutorialSecret, key, "", nil, nil); err == nil {
			t.Errorf("key %q should be rejected", key)
		}
	}
}

func TestModuleFetcherDefaults(t *testing.T) {
	fetcher, err := NewModuleFetcher(tutorialSecret, testutil.GenerateEncryptionKey(), "", nil, nil)
	if err != nil {
		t.Fatalf("NewModuleFetcher: %v", err)
	}
	if fetcher.baseURL != defaultModuleBaseURL {
		t.Errorf("base URL = %q, want %q", fetcher.baseURL, defaultModuleBaseURL)
	}
}
