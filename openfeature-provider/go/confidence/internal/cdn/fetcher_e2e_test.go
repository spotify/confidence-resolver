package cdn

import (
	"context"
	"os"
	"testing"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/host"
)

// Fetches the e2e credential's module from the production CDN and validates it with the host.
// Needs CONFIDENCE_CLIENT_SECRET and CONFIDENCE_CLIENT_ENCRYPTION_KEY.
func TestE2E_FetchProductionModule(t *testing.T) {
	secret, hexKey := os.Getenv("CONFIDENCE_CLIENT_SECRET"), os.Getenv("CONFIDENCE_CLIENT_ENCRYPTION_KEY")
	if secret == "" || hexKey == "" {
		t.Skip("CONFIDENCE_CLIENT_SECRET and CONFIDENCE_CLIENT_ENCRYPTION_KEY not set")
	}
	key, err := ParseKey(hexKey)
	if err != nil {
		t.Fatal(err)
	}
	f, err := NewFetcher(secret, key, "", nil)
	if err != nil {
		t.Fatal(err)
	}
	ctx := context.Background()
	m, err := f.Fetch(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if m.ETag == "" || m.LogToken == "" {
		t.Errorf("etag present=%v, log token present=%v", m.ETag != "", m.LogToken != "")
	}
	rt := host.NewRuntime(ctx, false)
	defer rt.Close(ctx)
	if _, err := rt.Compile(ctx, m.Wasm); err != nil {
		t.Fatalf("production module rejected: %v", err)
	}
}
