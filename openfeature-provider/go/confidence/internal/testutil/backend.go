// Package testutil fakes the Confidence backend for tests.
package testutil

import (
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"testing"

	"google.golang.org/protobuf/proto"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/s2wpb/wasmpb"
)

// Secret is the client secret of the shared test states.
const Secret = "test-secret"

// Module reads a compiled module from openfeature-provider/testdata/modules.
func Module(t testing.TB, name string) []byte {
	_, here, _, _ := runtime.Caller(0)
	b, err := os.ReadFile(filepath.Join(filepath.Dir(here), "../../../../testdata/modules", name))
	if err != nil {
		t.Fatal(err)
	}
	return b
}

// Backend serves one encrypted module and records flag-log batches.
type Backend struct {
	t   testing.TB
	Key []byte

	mu      sync.Mutex
	status  int
	module  []byte
	etag    string
	inm     []string
	batches []Batch

	CDN, Resolver, Ingest *httptest.Server
}

// Batch is one received flag-log request.
type Batch struct {
	Token string
	Body  []byte
}

func NewBackend(t testing.TB) *Backend {
	b := &Backend{t: t, Key: make([]byte, 32), status: http.StatusNotFound}
	_, _ = rand.Read(b.Key)
	b.CDN = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b.mu.Lock()
		defer b.mu.Unlock()
		inm := r.Header.Get("If-None-Match")
		b.inm = append(b.inm, inm)
		switch {
		case b.status != http.StatusOK:
			w.WriteHeader(b.status)
		case inm != "" && inm == b.etag:
			w.WriteHeader(http.StatusNotModified)
		default:
			w.Header().Set("ETag", b.etag)
			_, _ = w.Write(b.module)
		}
	}))
	b.Resolver = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost || r.URL.Path != "/v1/profile:register" {
			w.WriteHeader(http.StatusNotFound)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, `{"cdnRetryDelay":"0.050s"}`)
	}))
	b.Ingest = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		b.mu.Lock()
		b.batches = append(b.batches, Batch{strings.TrimPrefix(r.Header.Get("Authorization"), "Bearer "), body})
		b.mu.Unlock()
	}))
	t.Cleanup(b.CDN.Close)
	t.Cleanup(b.Resolver.Close)
	t.Cleanup(b.Ingest.Close)
	return b
}

// HexKey is Key hex-encoded, as customers configure it.
func (b *Backend) HexKey() string {
	const digits = "0123456789abcdef"
	out := make([]byte, 0, 64)
	for _, c := range b.Key {
		out = append(out, digits[c>>4], digits[c&15])
	}
	return string(out)
}

// Publish serves wasm, with token, under etag.
func (b *Backend) Publish(wasm []byte, token, etag string) {
	plain, err := proto.Marshal(&wasmpb.ClientModule{Module: wasm, LogToken: token})
	if err != nil {
		b.t.Fatal(err)
	}
	block, _ := aes.NewCipher(b.Key)
	aead, _ := cipher.NewGCM(block)
	nonce := make([]byte, aead.NonceSize())
	_, _ = rand.Read(nonce)
	b.mu.Lock()
	b.status, b.module, b.etag = http.StatusOK, aead.Seal(nonce, nonce, plain, nil), etag
	b.mu.Unlock()
}

// SetStatus makes the CDN answer with status instead of a module.
func (b *Backend) SetStatus(status int) {
	b.mu.Lock()
	b.status = status
	b.mu.Unlock()
}

// IfNoneMatch returns the If-None-Match header of every CDN request so far.
func (b *Backend) IfNoneMatch() []string {
	b.mu.Lock()
	defer b.mu.Unlock()
	return append([]string(nil), b.inm...)
}

// Batches returns the flag-log batches received so far.
func (b *Backend) Batches() []Batch {
	b.mu.Lock()
	defer b.mu.Unlock()
	return append([]Batch(nil), b.batches...)
}

// Logs decodes every received batch.
func (b *Backend) Logs() []*wasmpb.WriteFlagLogsRequest {
	var out []*wasmpb.WriteFlagLogsRequest
	for _, batch := range b.Batches() {
		var req wasmpb.WriteFlagLogsRequest
		if err := proto.Unmarshal(batch.Body, &req); err != nil {
			b.t.Fatal(err)
		}
		out = append(out, &req)
	}
	return out
}
