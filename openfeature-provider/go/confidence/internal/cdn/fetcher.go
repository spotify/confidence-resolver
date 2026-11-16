// Package cdn fetches a credential's compiled resolver module from the Confidence module CDN.
package cdn

import (
	"context"
	"crypto/aes"
	"crypto/cipher"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"net/http"
	"sync"

	"google.golang.org/protobuf/proto"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/s2wpb/wasmpb"
)

// DefaultBaseURL is the production module CDN.
const DefaultBaseURL = "https://confidence-resolver-module-cdn.spotifycdn.com"

// Module is a decrypted ClientModule. Wasm embeds the client secret: never log it or write it to disk.
type Module struct {
	Wasm     []byte
	LogToken string
	ETag     string
}

var (
	// ErrNotModified: the module is unchanged since the last accepted ETag.
	ErrNotModified = errors.New("module not modified")
	// ErrNotPublished: no module exists for this credential (yet). Retryable.
	ErrNotPublished = errors.New("no module published for this credential")
)

// TerminalError is a failure that retrying won't fix, such as a rejected credential or an artifact
// that doesn't decrypt.
type TerminalError struct{ Err error }

func (e *TerminalError) Error() string { return e.Err.Error() }
func (e *TerminalError) Unwrap() error { return e.Err }

// Fetcher polls one credential's module, using ETags for conditional requests.
type Fetcher struct {
	url    string
	aead   cipher.AEAD
	client *http.Client

	mu   sync.Mutex
	etag string
}

// ParseKey validates a hex-encoded AES-256 key. The error never includes the key.
func ParseKey(hexKey string) ([]byte, error) {
	if len(hexKey) != 64 {
		return nil, errors.New("encryption key must be 64 hexadecimal characters")
	}
	key, err := hex.DecodeString(hexKey)
	if err != nil {
		return nil, errors.New("encryption key must be hexadecimal")
	}
	return key, nil
}

// NewFetcher creates a fetcher for clientSecret. baseURL and client may be empty and nil.
func NewFetcher(clientSecret string, key []byte, baseURL string, client *http.Client) (*Fetcher, error) {
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, err
	}
	aead, err := cipher.NewGCM(block)
	if err != nil {
		return nil, err
	}
	if baseURL == "" {
		baseURL = DefaultBaseURL
	}
	if client == nil {
		client = http.DefaultClient
	}
	sum := sha256.Sum256([]byte(clientSecret))
	return &Fetcher{url: baseURL + "/modules/" + hex.EncodeToString(sum[:]) + ".wasm.enc", aead: aead, client: client}, nil
}

// Fetch downloads and decrypts the module. It returns ErrNotModified when the module matches the
// last accepted ETag. Call Accept once the module is installed, so that a module that fails to
// install is fetched again.
func (f *Fetcher) Fetch(ctx context.Context) (*Module, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, f.url, nil)
	if err != nil {
		return nil, err
	}
	f.mu.Lock()
	if f.etag != "" {
		req.Header.Set("If-None-Match", f.etag)
	}
	f.mu.Unlock()
	resp, err := f.client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	switch {
	case resp.StatusCode == http.StatusNotModified:
		return nil, ErrNotModified
	case resp.StatusCode == http.StatusNotFound:
		return nil, ErrNotPublished
	case resp.StatusCode == http.StatusRequestTimeout || resp.StatusCode == http.StatusTooManyRequests || resp.StatusCode >= 500:
		return nil, fmt.Errorf("module CDN: %s", resp.Status)
	case resp.StatusCode != http.StatusOK:
		return nil, &TerminalError{fmt.Errorf("module CDN: %s", resp.Status)}
	}
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, err
	}
	m, err := f.decrypt(body)
	if err != nil {
		return nil, &TerminalError{err}
	}
	m.ETag = resp.Header.Get("ETag")
	return m, nil
}

// Accept records etag as installed. Later fetches are conditional on it.
func (f *Fetcher) Accept(etag string) {
	f.mu.Lock()
	f.etag = etag
	f.mu.Unlock()
}

// decrypt opens nonce(12) || ciphertext || tag(16) with empty associated data and decodes the
// ClientModule inside.
func (f *Fetcher) decrypt(body []byte) (*Module, error) {
	n := f.aead.NonceSize()
	if len(body) < n+f.aead.Overhead() {
		return nil, errors.New("module artifact too short")
	}
	plain, err := f.aead.Open(nil, body[:n], body[n:], nil)
	if err != nil {
		return nil, errors.New("module artifact failed to decrypt (wrong encryption key?)")
	}
	var cm wasmpb.ClientModule
	if err := proto.Unmarshal(plain, &cm); err != nil {
		return nil, fmt.Errorf("module artifact: %w", err)
	}
	if len(cm.GetModule()) == 0 || cm.GetLogToken() == "" {
		return nil, errors.New("module artifact is missing its module or log token")
	}
	return &Module{Wasm: cm.GetModule(), LogToken: cm.GetLogToken()}, nil
}
