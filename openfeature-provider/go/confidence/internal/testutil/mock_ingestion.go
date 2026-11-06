package testutil

import (
	"net/http"
	"net/http/httptest"
	"sync"
	"testing"
	"time"
)

// IngestedBatch records one delivered log batch.
type IngestedBatch struct {
	Body        []byte
	Token       string
	ContentType string
}

// MockIngestion is a test double for the flag-log ingestion API: it accepts
// POST /v1/flagLogs:write and records bodies, bearer tokens and content
// types, with injectable failures.
type MockIngestion struct {
	t *testing.T

	mu       sync.Mutex
	batches  []IngestedBatch
	failWith int // non-zero: respond with this status

	delivered chan struct{} // signaled after each accepted batch

	server *httptest.Server
}

// NewMockIngestion starts the mock ingestion API.
func NewMockIngestion(t *testing.T) *MockIngestion {
	t.Helper()
	m := &MockIngestion{
		t:         t,
		delivered: make(chan struct{}, 1024),
	}
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/flagLogs:write", func(w http.ResponseWriter, r *http.Request) {
		m.mu.Lock()
		body := make([]byte, r.ContentLength)
		n, _ := r.Body.Read(body)
		body = body[:n]
		batch := IngestedBatch{
			Body:        body,
			Token:       r.Header.Get("Authorization"),
			ContentType: r.Header.Get("Content-Type"),
		}
		fail := m.failWith
		m.mu.Unlock()

		if fail != 0 {
			w.WriteHeader(fail)
			return
		}
		m.mu.Lock()
		m.batches = append(m.batches, batch)
		m.mu.Unlock()
		w.WriteHeader(http.StatusOK)
		select {
		case m.delivered <- struct{}{}:
		default:
		}
	})
	m.server = httptest.NewServer(mux)
	t.Cleanup(m.server.Close)
	return m
}

// URL returns the mock ingestion base URL.
func (m *MockIngestion) URL() string {
	return m.server.URL
}

// SetFail makes the API respond with the given status (0 restores success).
func (m *MockIngestion) SetFail(status int) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.failWith = status
}

// Batches returns the accepted batches so far.
func (m *MockIngestion) Batches() []IngestedBatch {
	m.mu.Lock()
	defer m.mu.Unlock()
	return append([]IngestedBatch(nil), m.batches...)
}

// AwaitDelivered blocks until at least n batches were accepted or the
// timeout passes, returning whether n were seen.
func (m *MockIngestion) AwaitDelivered(t *testing.T, n int, timeout time.Duration) bool {
	t.Helper()
	deadline := time.After(timeout)
	for {
		m.mu.Lock()
		count := len(m.batches)
		m.mu.Unlock()
		if count >= n {
			return true
		}
		select {
		case <-m.delivered:
		case <-deadline:
			m.mu.Lock()
			final := len(m.batches)
			m.mu.Unlock()
			return final >= n
		}
	}
}
