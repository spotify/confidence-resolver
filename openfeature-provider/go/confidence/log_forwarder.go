package confidence

import (
	"bytes"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"sync"
	"sync/atomic"
	"time"
)

// defaultIngestionBaseURL is the regionless base URL of the flag-log
// ingestion endpoint: epx-flags-ingestion's FlagLogIngestionService,
// publicly exposed at ingestion.confidence.dev and the regional eu/us
// pair (edge-config epx-flags-ingestion-perimeter) as
// POST /v1/flagLogs:write. The edge terminates TLS and passes
// application/x-protobuf bodies through; the service authenticates the
// compiler-signed Bearer log token itself.
const defaultIngestionBaseURL = "https://ingestion.confidence.dev"

// logForwarderDefaults. The per-send timeout matches the reference
// forwarder's 5s delivery deadline (epx-wasm-resolver's FlagLogForwarder).
const (
	defaultLogQueueDepth    = 256
	defaultLogRetryAttempts = 3
	defaultLogRetryBackoff  = 500 * time.Millisecond
	logForwardHTTPTimeout   = 5 * time.Second
)

// LogBatchStats reports the forwarder's delivery accounting.
type LogBatchStats struct {
	Sent    uint64 // batches accepted by ingestion
	Retried uint64 // attempts that were retried
	Dropped uint64 // batches dropped (queue full, retries exhausted, bad request)
}

// LogForwarder delivers log batches produced by the module to the ingestion
// API over HTTPS.
//
// Delivery is push-only and strictly best-effort: a batch is enqueued by the
// module's write_logs callback together with the wrapper's log token at
// delivery time, and a single background worker forwards batches in order.
// Batches from different wrappers never share a request; the token stays with
// its batch for the batch's whole life, including retries. Log delivery never
// blocks a resolve and never fails an evaluation: when the bounded queue is
// full or retries are exhausted the batch is dropped and counted.
type LogForwarder struct {
	baseURL      string
	httpClient   *http.Client
	logger       *slog.Logger
	retryBackoff time.Duration

	queues  chan logBatch
	drop    func(batch logBatch) // test hook, nil in production
	stopped chan struct{}
	closeWG sync.WaitGroup

	sent    atomic.Uint64
	retried atomic.Uint64
	dropped atomic.Uint64
}

type logBatch struct {
	body  []byte
	token string
}

// LogForwarderConfig configures a LogForwarder.
type LogForwarderConfig struct {
	// BaseURL of the ingestion API; defaults to the regionless host. Only
	// the base URL is configurable: the path and headers are fixed.
	BaseURL string
	// HTTPClient used for delivery. Optional.
	HTTPClient *http.Client
	// Logger. Optional.
	Logger *slog.Logger
	// QueueDepth bounds the pending batches. When full, new batches are
	// dropped. Zero means the default.
	QueueDepth int
	// RetryAttempts bounds retries per batch (transport errors, 5xx, 429).
	// Zero means the default. Retries are dropped, never queued
	// indefinitely.
	RetryAttempts int
	// RetryBackoff is the initial backoff between retries. Zero means the
	// default.
	RetryBackoff time.Duration
}

// NewLogForwarder starts the delivery worker.
func NewLogForwarder(cfg LogForwarderConfig) *LogForwarder {
	if cfg.BaseURL == "" {
		cfg.BaseURL = defaultIngestionBaseURL
	}
	if cfg.Logger == nil {
		cfg.Logger = slog.Default()
	}
	if cfg.HTTPClient == nil {
		cfg.HTTPClient = &http.Client{Timeout: logForwardHTTPTimeout}
	}
	if cfg.QueueDepth <= 0 {
		cfg.QueueDepth = defaultLogQueueDepth
	}
	if cfg.RetryAttempts <= 0 {
		cfg.RetryAttempts = defaultLogRetryAttempts
	}
	if cfg.RetryBackoff <= 0 {
		cfg.RetryBackoff = defaultLogRetryBackoff
	}
	f := &LogForwarder{
		baseURL:      cfg.BaseURL,
		httpClient:   cfg.HTTPClient,
		logger:       cfg.Logger,
		retryBackoff: cfg.RetryBackoff,
		queues:       make(chan logBatch, cfg.QueueDepth),
		stopped:      make(chan struct{}),
	}
	attempts := cfg.RetryAttempts
	f.closeWG.Add(1)
	go f.worker(attempts)
	return f
}

// Enqueue hands one batch to the forwarder, capturing the token that belongs
// to it. It is safe to call from the write_logs callback: it never blocks
// and never panics; a full queue drops the batch.
func (f *LogForwarder) Enqueue(body []byte, token string) {
	batch := logBatch{body: body, token: token}
	select {
	case f.queues <- batch:
	default:
		f.dropped.Add(1)
	}
}

// Stats reports delivery accounting so far.
func (f *LogForwarder) Stats() LogBatchStats {
	return LogBatchStats{
		Sent:    f.sent.Load(),
		Retried: f.retried.Load(),
		Dropped: f.dropped.Load(),
	}
}

// Shutdown stops accepting batches and drains what is already queued.
// Batches are delivered without retries during the drain; the call returns
// when the worker exits or the deadline passes, whichever comes first.
func (f *LogForwarder) Shutdown(deadline time.Duration) {
	select {
	case <-f.stopped:
	default:
		close(f.stopped)
	}

	done := make(chan struct{})
	go func() {
		f.closeWG.Wait()
		close(done)
	}()

	timer := time.NewTimer(deadline)
	defer timer.Stop()
	select {
	case <-done:
	case <-timer.C:
	}
}

// worker delivers batches in order until stopped, then drains the queue
// once without retries and exits.
func (f *LogForwarder) worker(attempts int) {
	defer f.closeWG.Done()
	for {
		select {
		case batch, ok := <-f.queues:
			if !ok {
				return
			}
			f.deliver(batch, attempts)
		case <-f.stopped:
			// Final drain: deliver what is queued with no retry budget, then
			// exit. The drain is bounded by the queue depth and one HTTP
			// timeout per batch.
			for {
				select {
				case batch, ok := <-f.queues:
					if !ok {
						return
					}
					f.deliver(batch, 1)
				default:
					return
				}
			}
		}
	}
}

// deliver forwards one batch, retrying transport errors, 5xx and 429
// responses up to the attempt budget; everything else is dropped.
func (f *LogForwarder) deliver(batch logBatch, attempts int) {
	for attempt := 1; ; attempt++ {
		err := f.post(batch)
		switch {
		case err == nil:
			f.sent.Add(1)
			return
		case attempt >= attempts, errors.Is(err, errIngestionRejected):
			// Retries exhausted, or the request is bad in a way retries
			// cannot fix: drop and count.
			f.dropped.Add(1)
			// The token is never logged; bodies are not logged at shared
			// levels either.
			f.logger.Warn("dropping log batch", "attempt", attempt, "error", err)
			return
		default:
			f.retried.Add(1)
			time.Sleep(f.retryBackoff * time.Duration(attempt))
		}
	}
}

// errIngestionRejected marks responses that retries cannot fix.
var errIngestionRejected = errors.New("ingestion rejected the batch")

func (f *LogForwarder) post(batch logBatch) error {
	req, err := http.NewRequest(http.MethodPost, f.baseURL+"/v1/flagLogs:write", bytes.NewReader(batch.body))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/x-protobuf")
	if batch.token != "" {
		req.Header.Set("Authorization", "Bearer "+batch.token)
	}

	resp, err := f.httpClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	_, _ = io.Copy(io.Discard, resp.Body)

	if resp.StatusCode >= 200 && resp.StatusCode < 300 {
		return nil
	}
	if resp.StatusCode == http.StatusTooManyRequests || resp.StatusCode >= 500 {
		return fmt.Errorf("ingestion returned %d", resp.StatusCode)
	}
	return errIngestionRejected
}
