package confidence

import (
	"context"
	"crypto/aes"
	"crypto/cipher"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"path"
	"sync/atomic"
	"time"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/compiler"
	"google.golang.org/protobuf/proto"
)

// defaultModuleBaseURL is the production CDN serving compiled client
// modules. The per-credential state CDN is a sibling host; the two coexist
// during migration and use different path shapes.
const defaultModuleBaseURL = "https://confidence-resolver-module-cdn.spotifycdn.com"

// moduleNotPublishedError reports that no module artifact exists for the
// credential yet (HTTP 404). It is not a failure: the provider keeps serving
// its last good module, or keeps polling before it has one.
type moduleNotPublishedError struct{ error }

func isModuleNotPublished(err error) bool {
	var notPublished *moduleNotPublishedError
	return errors.As(err, &notPublished)
}

// moduleUnchangedError reports a 304 Not Modified: the artifact the provider
// already holds is still current.
type moduleUnchangedError struct{ error }

func isModuleUnchanged(err error) bool {
	var unchanged *moduleUnchangedError
	return errors.As(err, &unchanged)
}

// FetchedModule is one accepted artifact: the decrypted wrapper and its
// CDN validators.
type FetchedModule struct {
	// Module is the raw, uncompressed WASM module.
	Module []byte
	// LogToken is the compiler-signed bearer token for the ingestion API.
	LogToken string
	// ETag of the accepted artifact, sent as If-None-Match on later polls.
	ETag string
}

// ModuleFetcher fetches and decrypts a credential's compiled client module
// from the module CDN.
type ModuleFetcher struct {
	clientSecret  string
	encryptionKey []byte
	baseURL       string
	httpClient    *http.Client
	logger        *slog.Logger
	etag          atomic.Value // string; ETag of the last accepted artifact
}

// NewModuleFetcher creates a fetcher for one credential. The encryption key
// must be the credential's hex-encoded AES-256 key; the same wire format
// (12-byte nonce || ciphertext || 16-byte tag, empty AAD) used for encrypted
// resolver state decrypts the module wrapper.
func NewModuleFetcher(clientSecret, encryptionKey string, baseURL string, logger *slog.Logger, httpClient *http.Client) (*ModuleFetcher, error) {
	if err := validateEncryptionKey(encryptionKey); err != nil {
		return nil, err
	}
	if baseURL == "" {
		baseURL = defaultModuleBaseURL
	}
	if logger == nil {
		logger = slog.Default()
	}
	if httpClient == nil {
		httpClient = &http.Client{Timeout: 30 * time.Second}
	}
	key, err := hex.DecodeString(encryptionKey)
	if err != nil {
		return nil, err
	}
	return &ModuleFetcher{
		clientSecret:  clientSecret,
		encryptionKey: key,
		baseURL:       baseURL,
		httpClient:    httpClient,
		logger:        logger,
	}, nil
}

// moduleObjectPath derives the artifact path from the exact UTF-8 bytes of
// the client secret: modules/<lowercase-hex-sha256>.wasm.enc. The secret is
// never trimmed, case-folded or otherwise normalised.
func moduleObjectPath(clientSecret string) string {
	hash := sha256.Sum256([]byte(clientSecret))
	return path.Join("modules", hex.EncodeToString(hash[:])+".wasm.enc")
}

// Fetch retrieves the credential's artifact. It returns:
//   - a FetchedModule when a new artifact was accepted,
//   - moduleUnchangedError when the CDN answered 304 (nothing to do),
//   - moduleNotPublishedError on 404 (keep the last good module),
//   - any other error on transport, decrypt or wrapper failures (keep the
//     last good module and retry on the next poll).
func (f *ModuleFetcher) Fetch(ctx context.Context) (*FetchedModule, error) {
	url := f.baseURL + "/" + moduleObjectPath(f.clientSecret)

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, err
	}
	// The CDN preserves origin ETags: the cheap steady-state path is a 304.
	if etag, ok := f.etag.Load().(string); ok && etag != "" {
		req.Header.Set("If-None-Match", etag)
	}

	resp, err := f.httpClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	switch resp.StatusCode {
	case http.StatusNotModified:
		return nil, &moduleUnchangedError{errors.New("module unchanged")}
	case http.StatusNotFound:
		// No module published (yet, or no longer) for this credential.
		return nil, &moduleNotPublishedError{errors.New("no module published for this credential")}
	case http.StatusOK:
	default:
		return nil, fmt.Errorf("unexpected status fetching module: %d", resp.StatusCode)
	}

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, err
	}
	if len(body) == 0 {
		return nil, errors.New("empty module artifact")
	}

	plaintext, err := decryptAesGcm(body, f.encryptionKey)
	if err != nil {
		return nil, fmt.Errorf("failed to decrypt module wrapper: %w", err)
	}

	wrapper := &compiler.ClientModule{}
	if err := proto.Unmarshal(plaintext, wrapper); err != nil {
		return nil, fmt.Errorf("failed to parse module wrapper: %w", err)
	}
	if len(wrapper.GetModule()) == 0 {
		return nil, errors.New("module wrapper has no module bytes")
	}
	if wrapper.GetLogToken() == "" {
		f.logger.Warn("module wrapper carries no log token; log delivery will fail until the next wrapper")
	}

	etag := resp.Header.Get("ETag")
	f.etag.Store(etag)
	return &FetchedModule{
		Module:   wrapper.GetModule(),
		LogToken: wrapper.GetLogToken(),
		ETag:     etag,
	}, nil
}

// decryptAesGcm decrypts the wire format shared by encrypted resolver state
// and module wrappers: 12-byte nonce || ciphertext || 16-byte tag, no key
// prefix, empty associated data.
func decryptAesGcm(data, keyBytes []byte) ([]byte, error) {
	block, err := aes.NewCipher(keyBytes)
	if err != nil {
		return nil, err
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return nil, err
	}
	nonceSize := gcm.NonceSize()
	if len(data) < nonceSize {
		return nil, fmt.Errorf("encrypted payload too short (missing nonce)")
	}
	return gcm.Open(nil, data[:nonceSize], data[nonceSize:], nil)
}

// validateEncryptionKey requires a hex-encoded AES-256 key (64 hex chars).
func validateEncryptionKey(key string) error {
	decoded, err := hex.DecodeString(key)
	if err != nil || len(decoded) != 32 {
		return fmt.Errorf("EncryptionKey is required and must contain exactly 64 hexadecimal characters")
	}
	return nil
}
