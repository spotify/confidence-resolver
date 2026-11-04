package testutil

import (
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"sync"
	"testing"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/compiler"
	"google.golang.org/protobuf/proto"
)

// LoadClientModuleFixture reads a compiled client module from the
// confidence package's testdata directory.
//
// The path is resolved relative to the working directory of the calling
// test (go test runs each package's tests with the package directory as
// the working directory), which keeps it correct both in a checkout and
// in Docker build stages where the source tree layout differs.
//
// A missing fixture fails the test: a silently skipped test is a false
// green.
func LoadClientModuleFixture(t *testing.T, name string) []byte {
	t.Helper()
	wd, err := os.Getwd()
	if err != nil {
		t.Fatalf("could not resolve working directory: %v", err)
	}
	dataPath := filepath.Join(wd, "testdata", name)
	data, err := os.ReadFile(dataPath)
	if err != nil {
		t.Fatalf("module fixture %s could not be loaded: %v", dataPath, err)
	}
	return data
}

// GenerateEncryptionKey returns a fresh hex-encoded AES-256 key. Integration
// tests wrap fixtures on the fly with a new key per run so no key is ever
// checked in.
func GenerateEncryptionKey() string {
	key := make([]byte, 32)
	if _, err := rand.Read(key); err != nil {
		panic(err)
	}
	return hex.EncodeToString(key)
}

// WrapClientModule builds the encrypted artifact the CDN serves: the
// ClientModule protobuf wrapper (module bytes + log token) encrypted with
// AES-256-GCM as nonce || ciphertext || tag with empty AAD.
func WrapClientModule(t *testing.T, module []byte, logToken string, hexKey string) []byte {
	t.Helper()
	wrapper := &compiler.ClientModule{
		Module:   module,
		LogToken: logToken,
	}
	plaintext, err := proto.Marshal(wrapper)
	if err != nil {
		t.Fatalf("marshal wrapper: %v", err)
	}
	key, err := hex.DecodeString(hexKey)
	if err != nil {
		t.Fatalf("decode key: %v", err)
	}
	block, err := aes.NewCipher(key)
	if err != nil {
		t.Fatalf("aes: %v", err)
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		t.Fatalf("gcm: %v", err)
	}
	nonce := make([]byte, gcm.NonceSize())
	if _, err := rand.Read(nonce); err != nil {
		t.Fatalf("nonce: %v", err)
	}
	return gcm.Seal(nonce, nonce, plaintext, nil)
}

// ModulePath returns the artifact path the CDN serves for a client secret:
// derived from the exact UTF-8 bytes, never trimmed or case-folded.
func ModulePath(clientSecret string) string {
	hash := sha256.Sum256([]byte(clientSecret))
	return fmt.Sprintf("/modules/%s.wasm.enc", hex.EncodeToString(hash[:]))
}

// ModuleCDNRequest records one artifact request the mock CDN served.
type ModuleCDNRequest struct {
	Path     string
	INMValue string // If-None-Match header ("" when absent)
}

// MockModuleCDN is a test double for the module CDN: it serves encrypted
// client-module artifacts at /modules/<secret-hash>.wasm.enc with ETag
// support, and can simulate 404s, outages, corrupt artifacts and
// replacements. The encryption key is decided by the test and shared with
// the fetcher under test.
type MockModuleCDN struct {
	t *testing.T

	mu        sync.Mutex
	artifacts map[string][]byte // path -> encrypted artifact
	etags     map[string]string // path -> etag
	status    int               // when non-zero, forced response status
	corrupt   bool              // serve garbage instead of the artifact
	serial    int

	requests []ModuleCDNRequest
	server   *httptest.Server
}

// NewMockModuleCDN starts a mock CDN serving no artifacts yet; use
// SetArtifact to install one for a credential.
func NewMockModuleCDN(t *testing.T) *MockModuleCDN {
	t.Helper()
	cdn := &MockModuleCDN{
		t:         t,
		artifacts: map[string][]byte{},
		etags:     map[string]string{},
	}
	mux := http.NewServeMux()
	mux.HandleFunc("/modules/", func(w http.ResponseWriter, r *http.Request) {
		cdn.handle(w, r)
	})
	cdn.server = httptest.NewServer(mux)
	t.Cleanup(cdn.server.Close)
	return cdn
}

func (c *MockModuleCDN) handle(w http.ResponseWriter, r *http.Request) {
	c.mu.Lock()
	defer c.mu.Unlock()

	c.requests = append(c.requests, ModuleCDNRequest{
		Path:     r.URL.Path,
		INMValue: r.Header.Get("If-None-Match"),
	})

	if c.status != 0 {
		w.WriteHeader(c.status)
		return
	}

	artifact, ok := c.artifacts[r.URL.Path]
	if !ok {
		w.WriteHeader(http.StatusNotFound)
		return
	}
	if c.corrupt {
		artifact = []byte("garbage-bytes-not-a-valid-encrypted-wrapper")
	}

	etag := c.etags[r.URL.Path]
	if etag != "" && r.Header.Get("If-None-Match") == etag {
		w.WriteHeader(http.StatusNotModified)
		return
	}
	w.Header().Set("ETag", etag)
	w.Header().Set("Content-Type", "application/octet-stream")
	if _, err := w.Write(artifact); err != nil {
		c.t.Errorf("write artifact: %v", err)
	}
}

// SetArtifact installs (or replaces) the artifact for a client secret,
// wrapping the module with the given log token and hex-encoded key.
func (c *MockModuleCDN) SetArtifact(clientSecret string, module []byte, logToken string, hexKey string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	p := ModulePath(clientSecret)
	c.artifacts[p] = WrapClientModule(c.t, module, logToken, hexKey)
	c.serial++
	c.etags[p] = fmt.Sprintf("mock-etag-%d", c.serial)
}

// RemoveArtifact removes the artifact for a client secret (404 afterwards).
func (c *MockModuleCDN) RemoveArtifact(clientSecret string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	delete(c.artifacts, ModulePath(clientSecret))
	delete(c.etags, ModulePath(clientSecret))
}

// SetStatus forces every request to answer with the given status code (0
// restores normal serving).
func (c *MockModuleCDN) SetStatus(status int) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.status = status
}

// SetCorrupt makes the server serve garbage instead of the artifact.
func (c *MockModuleCDN) SetCorrupt(corrupt bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.corrupt = corrupt
}

// Requests returns the artifact requests served so far.
func (c *MockModuleCDN) Requests() []ModuleCDNRequest {
	c.mu.Lock()
	defer c.mu.Unlock()
	return append([]ModuleCDNRequest(nil), c.requests...)
}

// URL returns the mock CDN base URL.
func (c *MockModuleCDN) URL() string {
	return c.server.URL
}

// HTTPClient returns an HTTP client wired to the mock server's in-memory
// transport (no real network traffic).
func (c *MockModuleCDN) HTTPClient() *http.Client {
	return c.server.Client()
}
