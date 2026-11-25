// Package cdn fetches a credential's compiled resolver module from the Confidence module CDN.
package cdn

import (
	"bytes"
	"context"
	"crypto/aes"
	"crypto/cipher"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strings"
	"sync"
	"time"

	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/durationpb"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/s2wpb/wasmpb"
)

// DefaultBaseURL is the production module CDN.
const DefaultBaseURL = "https://confidence-resolver-module-cdn.spotifycdn.com"

const (
	defaultResolverBaseURL = "https://resolver.confidence.dev"
	initialBackoff         = 200 * time.Millisecond
	maxBackoff             = 10 * time.Second
	registrationTimeout    = 10 * time.Second
)

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
	url                string
	aead               cipher.AEAD
	client             *http.Client
	registrationURL    string
	registrationBody   []byte
	registrationClient http.Client
	clientSecret       string
	now                func() time.Time

	mu   sync.Mutex
	etag string

	registered        bool
	registrationError *TerminalError
	nextAttempt       time.Time
	backoff           time.Duration
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

// NewFetcher creates a fetcher for clientSecret. Empty URLs and a nil client select defaults.
func NewFetcher(clientSecret string, key []byte, profile, baseURL, resolverBaseURL string, client *http.Client) (*Fetcher, error) {
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
	if resolverBaseURL == "" {
		resolverBaseURL = defaultResolverBaseURL
	}
	if client == nil {
		client = http.DefaultClient
	}
	registrationClient := *client
	registrationClient.CheckRedirect = func(*http.Request, []*http.Request) error {
		return http.ErrUseLastResponse
	}
	registrationBody, _ := json.Marshal(map[string]string{"profile": profile})
	sum := sha256.Sum256([]byte(clientSecret))
	return &Fetcher{
		url:  strings.TrimRight(baseURL, "/") + "/modules/" + hex.EncodeToString(sum[:]) + "/" + strings.ToLower(profile) + ".wasm.enc",
		aead: aead, client: client, clientSecret: clientSecret, now: time.Now,
		registrationURL:    strings.TrimRight(resolverBaseURL, "/") + "/v1/profile:register",
		registrationBody:   registrationBody,
		registrationClient: registrationClient,
	}, nil
}

// Fetch downloads and decrypts the module. It returns ErrNotModified when the module matches the
// last accepted ETag. Call Accept once the module is installed, so that a module that fails to
// install is fetched again.
func (f *Fetcher) Fetch(ctx context.Context) (*Module, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	if f.now().Before(f.nextAttempt) {
		return nil, ErrNotPublished
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, f.url, nil)
	if err != nil {
		return nil, err
	}
	if f.etag != "" {
		req.Header.Set("If-None-Match", f.etag)
	}
	resp, err := f.client.Do(req)
	if err != nil {
		return nil, err
	}
	if resp.StatusCode == http.StatusNotFound {
		resp.Body.Close() // Release the CDN connection before registering.
		return nil, f.onMissing(ctx)
	}
	defer resp.Body.Close()
	switch {
	case resp.StatusCode == http.StatusNotModified:
		f.clearRegistration()
		return nil, ErrNotModified
	case resp.StatusCode == http.StatusRequestTimeout || resp.StatusCode == http.StatusTooManyRequests || resp.StatusCode >= 500:
		return nil, fmt.Errorf("module CDN: HTTP %d", resp.StatusCode)
	case resp.StatusCode != http.StatusOK:
		return nil, &TerminalError{fmt.Errorf("module CDN: HTTP %d", resp.StatusCode)}
	}
	f.clearRegistration()
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

func (f *Fetcher) onMissing(ctx context.Context) error {
	if f.registrationError != nil {
		return f.registrationError
	}
	if !f.registered {
		delay, err := f.register(ctx)
		if err == nil {
			f.registered = true
			f.nextAttempt = f.now().Add(delay)
			f.backoff = 0
			return ErrNotPublished
		}
		var terminal *TerminalError
		if errors.As(err, &terminal) {
			f.registrationError = terminal
			return err
		}
		f.deferMissingPoll()
		return err
	}
	f.deferMissingPoll()
	return ErrNotPublished
}

func (f *Fetcher) deferMissingPoll() {
	f.backoff = min(maxBackoff, max(initialBackoff, 2*f.backoff))
	f.nextAttempt = f.now().Add(f.backoff)
}

func (f *Fetcher) clearRegistration() {
	f.registered = false
	f.registrationError = nil
	f.nextAttempt = time.Time{}
	f.backoff = 0
}

func (f *Fetcher) register(ctx context.Context) (time.Duration, error) {
	ctx, cancel := context.WithTimeout(ctx, registrationTimeout)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, f.registrationURL, bytes.NewReader(f.registrationBody))
	if err != nil {
		return 0, &TerminalError{errors.New("invalid module registration URL")}
	}
	req.Header.Set("Authorization", "ClientSecret "+f.clientSecret)
	req.Header.Set("Content-Type", "application/json")
	resp, err := f.registrationClient.Do(req)
	if err != nil {
		if ctx.Err() != nil {
			return 0, ctx.Err()
		}
		return 0, errors.New("module registration request failed")
	}
	defer resp.Body.Close()
	switch resp.StatusCode {
	case http.StatusOK:
		const maxAckBytes = 4096
		body, err := io.ReadAll(io.LimitReader(resp.Body, maxAckBytes+1))
		if err != nil {
			if ctx.Err() != nil {
				return 0, ctx.Err()
			}
			return 0, errors.New("reading module registration acknowledgement failed")
		}
		var ack struct {
			CDNRetryDelay json.RawMessage `json:"cdnRetryDelay"`
		}
		var delay durationpb.Duration
		if len(body) > maxAckBytes || json.Unmarshal(body, &ack) != nil ||
			len(ack.CDNRetryDelay) == 0 || ack.CDNRetryDelay[0] != '"' ||
			protojson.Unmarshal(ack.CDNRetryDelay, &delay) != nil {
			return 0, &TerminalError{errors.New("invalid module registration acknowledgement")}
		}
		retryDelay := delay.AsDuration()
		roundTrip := durationpb.New(retryDelay)
		if retryDelay < 0 || delay.Seconds != roundTrip.Seconds || delay.Nanos != roundTrip.Nanos {
			return 0, &TerminalError{errors.New("invalid module registration acknowledgement")}
		}
		return retryDelay, nil
	default:
		err := fmt.Errorf("module registration: HTTP %d", resp.StatusCode)
		if resp.StatusCode == http.StatusRequestTimeout || resp.StatusCode == http.StatusTooManyRequests || resp.StatusCode >= 500 {
			return 0, err
		}
		return 0, &TerminalError{err}
	}
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
