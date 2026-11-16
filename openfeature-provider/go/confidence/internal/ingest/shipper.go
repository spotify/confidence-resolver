// Package ingest forwards serialized WriteFlagLogsRequests to the Confidence flag-log ingestion API.
package ingest

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"sync"
	"sync/atomic"
	"time"
)

// DefaultBaseURL is the regionless production ingestion host (from the Go POC; unverified).
const DefaultBaseURL = "https://ingestion.confidence.dev"

const (
	path           = "/v1/flagLogs:write"
	attemptTimeout = 5 * time.Second
	maxAttempts    = 3
)

// Shipper sends batches from a bounded queue on a background goroutine. Batches the queue can't
// hold are dropped and counted rather than blocking the caller, which may be inside a WASM call.
type Shipper struct {
	url     string
	client  *http.Client
	backoff time.Duration
	queue   chan batch
	done    chan struct{}

	mu     sync.RWMutex // guards closed against concurrent Enqueue
	closed bool

	dropped, failed atomic.Uint64
	report          func(error)
}

type batch struct {
	body  []byte
	token string
}

// Config configures a Shipper. Zero values select defaults.
type Config struct {
	BaseURL   string
	Client    *http.Client
	QueueSize int         // batches; default 64
	Report    func(error) // delivery failures; default ignore
}

func New(cfg Config) *Shipper {
	if cfg.BaseURL == "" {
		cfg.BaseURL = DefaultBaseURL
	}
	if cfg.Client == nil {
		cfg.Client = http.DefaultClient
	}
	if cfg.QueueSize <= 0 {
		cfg.QueueSize = 64
	}
	if cfg.Report == nil {
		cfg.Report = func(error) {}
	}
	s := &Shipper{
		url: cfg.BaseURL + path, client: cfg.Client, backoff: 200 * time.Millisecond,
		queue: make(chan batch, cfg.QueueSize), done: make(chan struct{}), report: cfg.Report,
	}
	go s.run()
	return s
}

// Enqueue queues body for delivery with token, without blocking. It reports false if the batch
// was dropped because the queue is full or the shipper is closed.
func (s *Shipper) Enqueue(body []byte, token string) bool {
	s.mu.RLock()
	defer s.mu.RUnlock()
	if s.closed {
		s.dropped.Add(1)
		return false
	}
	select {
	case s.queue <- batch{body, token}:
		return true
	default:
		s.dropped.Add(1)
		return false
	}
}

// Dropped counts batches never sent because the queue was full or closed.
func (s *Shipper) Dropped() uint64 { return s.dropped.Load() }

// Failed counts batches given up on after a rejection or exhausted retries.
func (s *Shipper) Failed() uint64 { return s.failed.Load() }

// Close stops accepting batches and waits for the queue to drain, or for ctx to end.
func (s *Shipper) Close(ctx context.Context) error {
	s.mu.Lock()
	if !s.closed {
		s.closed = true
		close(s.queue)
	}
	s.mu.Unlock()
	select {
	case <-s.done:
		return nil
	case <-ctx.Done():
		return fmt.Errorf("log queue not drained: %w", ctx.Err())
	}
}

func (s *Shipper) run() {
	defer close(s.done)
	for b := range s.queue {
		if err := s.send(b); err != nil {
			s.failed.Add(1)
			s.report(err)
		}
	}
}

type retryable struct{ error }

func (s *Shipper) send(b batch) error {
	var err error
	for attempt := range maxAttempts {
		if attempt > 0 {
			time.Sleep(s.backoff << (attempt - 1))
		}
		if err = s.post(b); err == nil {
			return nil
		}
		if !errors.As(err, &retryable{}) {
			return err
		}
	}
	return err
}

func (s *Shipper) post(b batch) error {
	ctx, cancel := context.WithTimeout(context.Background(), attemptTimeout)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, s.url, bytes.NewReader(b.body))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/x-protobuf")
	req.Header.Set("Authorization", "Bearer "+b.token)
	resp, err := s.client.Do(req)
	if err != nil {
		return retryable{fmt.Errorf("flag logs: %w", err)}
	}
	defer resp.Body.Close()
	_, _ = io.Copy(io.Discard, resp.Body)
	switch {
	case resp.StatusCode < 300:
		return nil
	case resp.StatusCode == http.StatusTooManyRequests || resp.StatusCode >= 500:
		return retryable{fmt.Errorf("flag logs: %s", resp.Status)}
	default:
		return fmt.Errorf("flag logs rejected: %s", resp.Status)
	}
}
