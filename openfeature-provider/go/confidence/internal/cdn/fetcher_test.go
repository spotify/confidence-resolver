package cdn

import (
	"context"
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"google.golang.org/protobuf/proto"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/s2wpb/wasmpb"
)

const (
	secret      = "test-secret"
	testProfile = "TEST_MODULE"
)

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
	client := &http.Client{Transport: roundTripFunc(func(r *http.Request) (*http.Response, error) {
		w := httptest.NewRecorder()
		switch r.URL.Host {
		case "cdn.test":
			c.ServeHTTP(w, r)
		case "resolver.test":
			_, _ = io.WriteString(w, `{"cdnRetryDelay":"10s"}`)
		default:
			t.Fatalf("unexpected HTTP request: %s", r.URL)
		}
		return w.Result(), nil
	})}
	f, err := NewFetcher(secret, key, testProfile, "https://cdn.test", "https://resolver.test", client)
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
	if c.path != "/modules/"+hex.EncodeToString(sum[:])+"/test_module.wasm.enc" {
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

type roundTripFunc func(*http.Request) (*http.Response, error)

func (rt roundTripFunc) RoundTrip(r *http.Request) (*http.Response, error) { return rt(r) }

// Mock both origins and advance a fake monotonic clock instead of waiting on retry timers.
func registrationFixture(t *testing.T, resolverURL string, register roundTripFunc) (*Fetcher, *cdn, *time.Time, *[]string) {
	t.Helper()
	key := newKey(t)
	c := &cdn{status: 404, body: nil}
	now := time.Now()
	var calls []string
	client := &http.Client{
		CheckRedirect: func(*http.Request, []*http.Request) error {
			t.Error("registration used the supplied client's redirect policy")
			return nil
		},
		Transport: roundTripFunc(func(r *http.Request) (*http.Response, error) {
			switch r.URL.Host {
			case "cdn.test":
				calls = append(calls, "GET")
				if r.Method != http.MethodGet || r.Header.Get("Authorization") != "" {
					t.Errorf("unexpected CDN method or Authorization header")
				}
				w := httptest.NewRecorder()
				c.ServeHTTP(w, r)
				resp := w.Result()
				resp.Status = "untrusted reason: " + secret
				return resp, nil
			case "resolver.test", "resolver.confidence.dev":
				calls = append(calls, "POST")
				if r.Method != http.MethodPost || r.URL.Path != "/v1/profile:register" ||
					r.Header.Get("Authorization") != "ClientSecret "+secret || r.Header.Get("Content-Type") != "application/json" {
					t.Error("incorrect registration request method, path or headers")
				}
				body, err := io.ReadAll(r.Body)
				_ = r.Body.Close()
				if err != nil || string(body) != `{"profile":"TEST_MODULE"}` {
					t.Errorf("incorrect registration request body: %q (%v)", body, err)
				}
				return register(r)
			default:
				t.Fatalf("unexpected HTTP request: %s", r.URL)
				return nil, nil
			}
		}),
	}
	f, err := NewFetcher(secret, key, testProfile, "https://cdn.test/", resolverURL, client)
	if err != nil {
		t.Fatal(err)
	}
	f.now = func() time.Time { return now }
	return f, c, &now, &calls
}

func response(status int, body string) *http.Response {
	w := httptest.NewRecorder()
	w.WriteHeader(status)
	_, _ = io.WriteString(w, body)
	return w.Result()
}

func TestRegistrationDelayBackoffAndReset(t *testing.T) {
	for _, published := range []int{http.StatusOK, http.StatusNotModified} {
		t.Run(http.StatusText(published), func(t *testing.T) {
			f, c, now, calls := registrationFixture(t, "https://resolver.test/", func(r *http.Request) (*http.Response, error) {
				if r.URL.Host != "resolver.test" {
					t.Errorf("registration ignored resolver override: %s", r.URL.Host)
				}
				return response(200, `{"cdnRetryDelay":"10s"}`), nil
			})
			poll := func(wantErr error, wantCalls string) {
				t.Helper()
				*calls = nil
				_, err := f.Fetch(context.Background())
				if !errors.Is(err, wantErr) || strings.Join(*calls, " ") != wantCalls {
					t.Fatalf("Fetch: err=%v calls=%v, want %v and %q", err, *calls, wantErr, wantCalls)
				}
			}
			poll(ErrNotPublished, "GET POST")
			for _, delay := range []time.Duration{10 * time.Second, 200 * time.Millisecond, 400 * time.Millisecond,
				800 * time.Millisecond, 1600 * time.Millisecond, 3200 * time.Millisecond, 6400 * time.Millisecond, 10 * time.Second} {
				*now = now.Add(delay - time.Nanosecond)
				poll(ErrNotPublished, "")
				*now = now.Add(time.Nanosecond)
				poll(ErrNotPublished, "GET")
			}
			*now = now.Add(10 * time.Second)
			c.status = published
			if published == http.StatusOK {
				plain, _ := proto.Marshal(&wasmpb.ClientModule{Module: []byte("wasm"), LogToken: "jwt"})
				nonce := make([]byte, f.aead.NonceSize())
				c.body = f.aead.Seal(nonce, nonce, plain, nil)
				poll(nil, "GET")
			} else {
				poll(ErrNotModified, "GET")
			}
			c.status = http.StatusNotFound
			poll(ErrNotPublished, "GET POST")
			*now = now.Add(10 * time.Second)
			poll(ErrNotPublished, "GET")
			*now = now.Add(200 * time.Millisecond)
			poll(ErrNotPublished, "GET")
		})
	}
}

func TestRegistrationRetriesOnlyAfterAnotherCDNMiss(t *testing.T) {
	attempt := 0
	f, c, now, calls := registrationFixture(t, "", func(r *http.Request) (*http.Response, error) {
		if r.URL.Host != "resolver.confidence.dev" {
			t.Errorf("wrong default resolver host: %s", r.URL.Host)
		}
		attempt++
		switch attempt {
		case 1:
			return nil, errors.New("transport leaked " + secret)
		case 2:
			return response(429, secret), nil
		default:
			return response(200, `{"cdnRetryDelay":"0s"}`), nil
		}
	})
	poll := func(wantCalls, wantError string) {
		t.Helper()
		*calls = nil
		_, err := f.Fetch(context.Background())
		var terminal *TerminalError
		if err == nil || errors.As(err, &terminal) || !strings.Contains(err.Error(), wantError) || strings.Contains(err.Error(), secret) ||
			strings.Join(*calls, " ") != wantCalls {
			t.Fatalf("Fetch: err=%v calls=%v, want retryable %q and %q", err, *calls, wantError, wantCalls)
		}
	}
	poll("GET POST", "registration request failed")
	poll("", ErrNotPublished.Error())
	*now = now.Add(200 * time.Millisecond)
	c.status = 503
	poll("GET", "module CDN: HTTP 503")
	c.status = 404
	poll("GET POST", "HTTP 429")
	*now = now.Add(200 * time.Millisecond)
	poll("", ErrNotPublished.Error())
	*now = now.Add(200 * time.Millisecond)
	poll("GET POST", ErrNotPublished.Error())
	poll("GET", ErrNotPublished.Error())
}

func TestRegistrationTerminalFailures(t *testing.T) {
	for _, tc := range []struct {
		name   string
		status int
		body   string
	}{
		{"redirect", 307, secret},
		{"forbidden", 403, secret},
		{"unexpected success status", 201, `{"cdnRetryDelay":"10s"}`},
		{"missing delay", 200, `{}`},
		{"null delay", 200, `{"cdnRetryDelay":null}`},
		{"invalid delay", 200, `{"cdnRetryDelay":"` + secret + `"}`},
		{"negative delay", 200, `{"cdnRetryDelay":"-1s"}`},
		{"overflowing delay", 200, `{"cdnRetryDelay":"10000000000s"}`},
	} {
		t.Run(tc.name, func(t *testing.T) {
			f, _, _, calls := registrationFixture(t, "https://resolver.test", func(*http.Request) (*http.Response, error) {
				r := response(tc.status, tc.body)
				r.Header.Set("Location", "https://redirect.test/"+secret)
				return r, nil
			})
			for range 2 {
				_, err := f.Fetch(context.Background())
				var terminal *TerminalError
				if !errors.As(err, &terminal) || strings.Contains(err.Error(), secret) || strings.Contains(err.Error(), "resolver.test") {
					t.Fatalf("Fetch: expected a sanitized terminal error, got %v", err)
				}
			}
			if got := strings.Join(*calls, " "); got != "GET POST GET" {
				t.Errorf("requests: %s", got)
			}
		})
	}
}

func TestRegistrationUsesFetchContext(t *testing.T) {
	ctx, cancel := context.WithDeadline(context.Background(), time.Now().Add(time.Second))
	defer cancel()
	f, _, _, _ := registrationFixture(t, "", func(r *http.Request) (*http.Response, error) {
		got, _ := r.Context().Deadline()
		want, _ := ctx.Deadline()
		if !got.Equal(want) {
			t.Errorf("registration deadline: %v, want %v", got, want)
		}
		cancel()
		return nil, r.Context().Err()
	})
	if _, err := f.Fetch(ctx); !errors.Is(err, context.Canceled) {
		t.Fatalf("Fetch error: %v, want context cancellation", err)
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
