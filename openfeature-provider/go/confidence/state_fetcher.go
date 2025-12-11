package confidence

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"sync/atomic"
	"time"

	admin "github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/admin"
	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/wasm"
	"google.golang.org/protobuf/proto"
)

// StateProvider is an interface for providing resolver state and account ID
type StateProvider interface {
	Provide(ctx context.Context) ([]byte, string, error)
}

// FlagsAdminStateFetcher fetches and updates the resolver state from the CDN
type FlagsAdminStateFetcher struct {
	clientSecret     string
	etag             atomic.Value // stores string
	rawResolverState atomic.Value // stores []byte
	accountID        atomic.Value // stores string
	HTTPClient       *http.Client // Exported for testing
	logger           *slog.Logger
}

// Compile-time interface conformance check
var _ StateProvider = (*FlagsAdminStateFetcher)(nil)

// NewFlagsAdminStateFetcher creates a new FlagsAdminStateFetcher
func NewFlagsAdminStateFetcher(
	clientSecret string,
	logger *slog.Logger,
) *FlagsAdminStateFetcher {
	return NewFlagsAdminStateFetcherWithTransport(clientSecret, logger, http.DefaultTransport)
}

// NewFlagsAdminStateFetcherWithTransport creates a new FlagsAdminStateFetcher with a custom HTTP transport.
func NewFlagsAdminStateFetcherWithTransport(
	clientSecret string,
	logger *slog.Logger,
	transport http.RoundTripper,
) *FlagsAdminStateFetcher {
	f := &FlagsAdminStateFetcher{
		clientSecret: clientSecret,
		logger:       logger,
		HTTPClient: &http.Client{
			Timeout:   30 * time.Second,
			Transport: transport,
		},
	}
	// Initialize with empty state
	emptyState := &admin.ResolverState{}
	if b, err := proto.Marshal(emptyState); err == nil {
		f.rawResolverState.Store(b)
	}

	return f
}

// GetRawState returns the current raw resolver state
func (f *FlagsAdminStateFetcher) GetRawState() []byte {
	if state := f.rawResolverState.Load(); state != nil {
		return state.([]byte)
	}
	return nil
}

// GetAccountID returns the account ID
func (f *FlagsAdminStateFetcher) GetAccountID() string {
	if accountID := f.accountID.Load(); accountID != nil {
		return accountID.(string)
	}
	return ""
}

// Reload fetches and updates the state if it has changed
func (f *FlagsAdminStateFetcher) Reload(ctx context.Context) error {
	return f.fetchAndUpdateStateIfChanged(ctx)
}

// Provide implements the StateProvider interface
// Returns the latest resolver state and account ID, fetching it if needed
// On error, returns cached state (if available) to maintain availability
func (f *FlagsAdminStateFetcher) Provide(ctx context.Context) ([]byte, string, error) {
	// Try to fetch the latest state
	err := f.Reload(ctx)
	// Always return the current state and accountID (cached or fresh)
	// This ensures availability even if fetch fails
	return f.GetRawState(), f.GetAccountID(), err
}

// fetchAndUpdateStateIfChanged fetches the state from the CDN if it has changed
func (f *FlagsAdminStateFetcher) fetchAndUpdateStateIfChanged(ctx context.Context) error {
	// Build CDN URL using SHA256 hash of client secret
	hash := sha256.Sum256([]byte(f.clientSecret))
	hashHex := hex.EncodeToString(hash[:])
	cdnURL := "https://confidence-resolver-state-cdn.spotifycdn.com/" + hashHex

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, cdnURL, nil)
	if err != nil {
		return err
	}

	// Add If-None-Match header if we have a previous ETag
	if previousEtag := f.etag.Load(); previousEtag != nil {
		req.Header.Set("If-None-Match", previousEtag.(string))
	}

	resp, err := f.HTTPClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	// Check if content was modified
	if resp.StatusCode == http.StatusNotModified {
		// Not modified, nothing to update
		return nil
	}

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("unexpected status code: %d", resp.StatusCode)
	}

	// Read the new state
	bytes, err := io.ReadAll(resp.Body)
	if err != nil {
		return err
	}

	// Parse SetResolverStateRequest
	stateRequest := &wasm.SetResolverStateRequest{}
	if err := proto.Unmarshal(bytes, stateRequest); err != nil {
		return fmt.Errorf("failed to unmarshal SetResolverStateRequest: %w", err)
	}

	// Extract account ID and state bytes
	f.accountID.Store(stateRequest.AccountId)

	// Get and store the new ETag
	etag := resp.Header.Get("ETag")
	f.etag.Store(etag)

	// Update the raw state (state is already in bytes format)
	f.rawResolverState.Store(stateRequest.State)

	f.logger.Debug("Loaded resolver state", "etag", etag, "account", stateRequest.AccountId)

	return nil
}
