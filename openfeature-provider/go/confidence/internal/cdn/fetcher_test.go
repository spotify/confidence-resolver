package cdn

import (
	"context"
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"google.golang.org/protobuf/proto"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/s2wpb/wasmpb"
)

const secret = "test-secret"

func newKey(t *testing.T) []byte {
	k := make([]byte, 32)
	if _, err := rand.Read(k); err != nil {
		t.Fatal(err)
	}
	return k
}

func encrypt(t *testing.T, key []byte, m *wasmpb.ClientModule) []byte {
	plain, err := proto.Marshal(m)
	if err != nil {
		t.Fatal(err)
	}
	block, _ := aes.NewCipher(key)
	aead, _ := cipher.NewGCM(block)
	nonce := make([]byte, aead.NonceSize())
	_, _ = rand.Read(nonce)
	return aead.Seal(nonce, nonce, plain, nil)
}

type cdn struct {
	status  int
	body    []byte
	etag    string
	lastINM string
	path    string
}

func (c *cdn) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	c.path, c.lastINM = r.URL.Path, r.Header.Get("If-None-Match")
	if c.etag != "" && c.lastINM == c.etag {
		w.WriteHeader(http.StatusNotModified)
		return
	}
	if c.etag != "" {
		w.Header().Set("ETag", c.etag)
	}
	w.WriteHeader(c.status)
	_, _ = w.Write(c.body)
}

func setup(t *testing.T, c *cdn, key []byte) *Fetcher {
	srv := httptest.NewServer(c)
	t.Cleanup(srv.Close)
	f, err := NewFetcher(secret, key, srv.URL, srv.Client())
	if err != nil {
		t.Fatal(err)
	}
	return f
}

func TestFetchDecryptsAndUsesETags(t *testing.T) {
	key := newKey(t)
	c := &cdn{status: 200, etag: `"v1"`, body: encrypt(t, key, &wasmpb.ClientModule{Module: []byte("\x00asm"), LogToken: "jwt"})}
	f := setup(t, c, key)
	ctx := context.Background()

	m, err := f.Fetch(ctx)
	if err != nil {
		t.Fatal(err)
	}
	sum := sha256.Sum256([]byte(secret))
	if c.path != "/modules/"+hex.EncodeToString(sum[:])+".wasm.enc" {
		t.Errorf("path %s", c.path)
	}
	if string(m.Wasm) != "\x00asm" || m.LogToken != "jwt" || m.ETag != `"v1"` {
		t.Errorf("module %+v", m)
	}

	// Not accepted yet (e.g. failed to install): fetch again unconditionally.
	if _, err := f.Fetch(ctx); err != nil || c.lastINM != "" {
		t.Errorf("refetch before Accept: err=%v If-None-Match=%q", err, c.lastINM)
	}
	f.Accept(m.ETag)
	if _, err := f.Fetch(ctx); !errors.Is(err, ErrNotModified) || c.lastINM != `"v1"` {
		t.Errorf("after Accept: err=%v If-None-Match=%q", err, c.lastINM)
	}
}

func TestFetchClassifiesFailures(t *testing.T) {
	key := newKey(t)
	good := encrypt(t, key, &wasmpb.ClientModule{Module: []byte("wasm"), LogToken: "jwt"})
	tampered := append([]byte{}, good...)
	tampered[len(tampered)-1] ^= 1
	for name, tc := range map[string]struct {
		c        cdn
		key      []byte
		retry    bool
		terminal bool
		is       error
	}{
		"not published": {c: cdn{status: 404}, is: ErrNotPublished},
		"server error":  {c: cdn{status: 503}, retry: true},
		"rate limited":  {c: cdn{status: 429}, retry: true},
		"forbidden":     {c: cdn{status: 403}, terminal: true},
		"wrong key":     {c: cdn{status: 200, body: good}, key: newKey(t), terminal: true},
		"tampered":      {c: cdn{status: 200, body: tampered}, terminal: true},
		"truncated":     {c: cdn{status: 200, body: good[:10]}, terminal: true},
		"no log token":  {c: cdn{status: 200, body: encrypt(t, key, &wasmpb.ClientModule{Module: []byte("wasm")})}, terminal: true},
	} {
		t.Run(name, func(t *testing.T) {
			k := key
			if tc.key != nil {
				k = tc.key
			}
			c := tc.c
			_, err := setup(t, &c, k).Fetch(context.Background())
			var term *TerminalError
			switch {
			case err == nil:
				t.Fatal("expected an error")
			case tc.is != nil && !errors.Is(err, tc.is):
				t.Errorf("err = %v, want %v", err, tc.is)
			case errors.As(err, &term) != tc.terminal:
				t.Errorf("terminal = %v, want %v (%v)", !tc.terminal, tc.terminal, err)
			}
			if strings.Contains(err.Error(), hex.EncodeToString(k)) {
				t.Error("error leaks the key")
			}
		})
	}
}

func TestParseKey(t *testing.T) {
	valid := strings.Repeat("aB", 32)
	if _, err := ParseKey(valid); err != nil {
		t.Errorf("valid key: %v", err)
	}
	for _, bad := range []string{"", strings.Repeat("a", 63), strings.Repeat("g", 64), " " + valid[1:]} {
		if _, err := ParseKey(bad); err == nil {
			t.Errorf("ParseKey(%q) accepted", bad)
		}
	}
}
