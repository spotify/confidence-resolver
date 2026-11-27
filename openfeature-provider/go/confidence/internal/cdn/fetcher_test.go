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
		case "resolver.confidence.dev":
			w.WriteHeader(http.StatusOK)
		default:
			t.Fatalf("unexpected HTTP request: %s", r.URL)
		}
		return w.Result(), nil
	})}
	f, err := NewFetcher(secret, key, testProfile, "https://cdn.test", "", client)
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

func registrationFixture(t *testing.T, register roundTripFunc) (*Fetcher, *cdn, *[]string) {
	t.Helper()
	key := newKey(t)
	c := &cdn{status: http.StatusNotFound}
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
			case "resolver.confidence.dev":
				calls = append(calls, "POST")
				if r.Method != http.MethodPost || r.URL.String() != "https://resolver.confidence.dev/v1/profile:register" ||
					r.Header.Get("Authorization") != "ClientSecret "+secret || r.Header.Get("Content-Type") != "application/json" {
					t.Error("incorrect registration request method, URL or headers")
				}
				if r.GetBody != nil {
					t.Error("registration body must not be replayable")
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
	f, err := NewFetcher(secret, key, testProfile, "https://cdn.test/", "", client)
	if err != nil {
		t.Fatal(err)
	}
	return f, c, &calls
}

func response(status int, body string) *http.Response {
	w := httptest.NewRecorder()
	w.WriteHeader(status)
	_, _ = io.WriteString(w, body)
	return w.Result()
}

func TestRegistrationAcknowledgementIgnoresBody(t *testing.T) {
	for name, body := range map[string]string{
		"empty body":      "",
		"malformed body":  "not JSON",
		"delay ignored":   `{"cdnRetryDelay":"10s"}`,
		"malformed delay": `{"cdnRetryDelay":null}`,
	} {
		t.Run(name, func(t *testing.T) {
			f, _, calls := registrationFixture(t, func(*http.Request) (*http.Response, error) {
				return response(http.StatusOK, body), nil
			})
			for range 2 {
				if _, err := f.Fetch(context.Background()); !errors.Is(err, ErrNotPublished) {
					t.Fatalf("Fetch: got %v, want ErrNotPublished", err)
				}
			}
			if got := strings.Join(*calls, " "); got != "GET POST GET" {
				t.Errorf("requests: got %q, want immediate CDN poll without another POST", got)
			}
		})
	}
}

func TestRegistrationSuccessSurvivesCDNHits(t *testing.T) {
	for _, published := range []int{http.StatusOK, http.StatusNotModified} {
		t.Run(http.StatusText(published), func(t *testing.T) {
			f, c, calls := registrationFixture(t, func(*http.Request) (*http.Response, error) {
				return response(http.StatusNoContent, ""), nil
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
			for range 3 {
				poll(ErrNotPublished, "GET")
			}
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
			for range 3 {
				poll(ErrNotPublished, "GET")
			}
		})
	}
}

func TestRegistrationRetriesOnCDNMissUntilSuccess(t *testing.T) {
	for _, tc := range []struct {
		name   string
		status int
		err    error
		want   string
	}{
		{"transport error", 0, errors.New("transport leaked " + secret), "module registration request failed"},
		{"timeout", 0, context.DeadlineExceeded, "module registration request timed out"},
		{"redirect to GET", http.StatusFound, nil, "module registration: HTTP 302"},
		{"redirect preserving POST", http.StatusTemporaryRedirect, nil, "module registration: HTTP 307"},
		{"forbidden", http.StatusForbidden, nil, "module registration: HTTP 403"},
		{"rate limited", http.StatusTooManyRequests, nil, "module registration: HTTP 429"},
		{"unavailable", http.StatusServiceUnavailable, nil, "module registration: HTTP 503"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			attempts := 0
			f, c, calls := registrationFixture(t, func(*http.Request) (*http.Response, error) {
				attempts++
				if attempts == 3 {
					return response(http.StatusAccepted, ""), nil
				}
				if tc.err != nil {
					return nil, tc.err
				}
				r := response(tc.status, secret)
				r.Status = "untrusted reason: " + secret
				r.Header.Set("Location", "https://redirect.test/"+secret)
				return r, nil
			})
			poll := func(wantErr error, wantMessage, wantCalls string) {
				t.Helper()
				*calls = nil
				_, err := f.Fetch(context.Background())
				var terminal *TerminalError
				if !errors.Is(err, wantErr) || errors.As(err, &terminal) || err.Error() != wantMessage || strings.Join(*calls, " ") != wantCalls {
					t.Fatalf("Fetch: err=%v calls=%v, want %q and %q", err, *calls, wantMessage, wantCalls)
				}
			}
			failure := ErrNotPublished.Error() + ": " + tc.want
			poll(ErrNotPublished, failure, "GET POST")
			poll(ErrNotPublished, failure, "GET POST")
			c.status = http.StatusNotModified
			poll(ErrNotModified, ErrNotModified.Error(), "GET")
			c.status = http.StatusNotFound
			poll(ErrNotPublished, ErrNotPublished.Error(), "GET POST")
			poll(ErrNotPublished, ErrNotPublished.Error(), "GET")
		})
	}
}

func TestRegistrationSkipsPOSTWhenFetchCanceledAfterCDNMiss(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	f, _, calls := registrationFixture(t, func(*http.Request) (*http.Response, error) {
		return response(http.StatusOK, ""), nil
	})
	transport := f.client.Transport
	f.client.Transport = roundTripFunc(func(r *http.Request) (*http.Response, error) {
		resp, err := transport.RoundTrip(r)
		cancel()
		return resp, err
	})
	if _, err := f.Fetch(ctx); !errors.Is(err, context.Canceled) || strings.Join(*calls, " ") != "GET" {
		t.Fatalf("canceled Fetch: err=%v calls=%v, want cancellation and no POST", err, *calls)
	}
	f.client.Transport = transport
	if _, err := f.Fetch(context.Background()); !errors.Is(err, ErrNotPublished) || strings.Join(*calls, " ") != "GET GET POST" {
		t.Fatalf("next Fetch: err=%v calls=%v, want ErrNotPublished and first POST", err, *calls)
	}
}

func TestRegistrationUsesFetchContext(t *testing.T) {
	ctx, cancel := context.WithDeadline(context.Background(), time.Now().Add(time.Second))
	defer cancel()
	f, _, _ := registrationFixture(t, func(r *http.Request) (*http.Response, error) {
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
