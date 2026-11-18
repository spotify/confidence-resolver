package ingest

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"sync"
	"testing"
	"time"
)

type server struct {
	mu       sync.Mutex
	statuses []int // served in order; 200 once exhausted
	got      []request
	block    chan struct{}
}

type request struct {
	path, auth, contentType, body string
}

func (s *server) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	if s.block != nil {
		<-s.block
	}
	body, _ := io.ReadAll(r.Body)
	s.mu.Lock()
	s.got = append(s.got, request{r.URL.Path, r.Header.Get("Authorization"), r.Header.Get("Content-Type"), string(body)})
	status := 200
	if len(s.statuses) > 0 {
		status, s.statuses = s.statuses[0], s.statuses[1:]
	}
	s.mu.Unlock()
	w.WriteHeader(status)
}

func newShipper(t *testing.T, s *server, queue int) *Shipper {
	srv := httptest.NewServer(s)
	t.Cleanup(srv.Close)
	sh := New(Config{BaseURL: srv.URL, Client: srv.Client(), QueueSize: queue})
	sh.backoff = time.Millisecond
	return sh
}

func TestShipsVerbatimWithToken(t *testing.T) {
	s := &server{}
	sh := newShipper(t, s, 0)
	sh.Enqueue([]byte("batch-1"), "tok-a")
	sh.Enqueue([]byte("batch-2"), "tok-b")
	if err := sh.Close(context.Background()); err != nil {
		t.Fatal(err)
	}
	want := []request{
		{"/v1/flagLogs:write", "Bearer tok-a", "application/x-protobuf", "batch-1"},
		{"/v1/flagLogs:write", "Bearer tok-b", "application/x-protobuf", "batch-2"},
	}
	if len(s.got) != 2 || s.got[0] != want[0] || s.got[1] != want[1] {
		t.Errorf("got %+v", s.got)
	}
}

func TestRetries(t *testing.T) {
	for name, tc := range map[string]struct {
		statuses []int
		requests int
		failed   uint64
	}{
		"5xx then ok":        {[]int{503, 500}, 3, 0},
		"429 then ok":        {[]int{429}, 2, 0},
		"exhausted":          {[]int{503, 503, 503}, 3, 1},
		"4xx is not retried": {[]int{400}, 1, 1},
	} {
		t.Run(name, func(t *testing.T) {
			s := &server{statuses: tc.statuses}
			sh := newShipper(t, s, 0)
			sh.Enqueue([]byte("b"), "t")
			if err := sh.Close(context.Background()); err != nil {
				t.Fatal(err)
			}
			if len(s.got) != tc.requests || sh.Failed() != tc.failed {
				t.Errorf("requests=%d failed=%d, want %d %d", len(s.got), sh.Failed(), tc.requests, tc.failed)
			}
		})
	}
}

func TestFullQueueDropsWithoutBlocking(t *testing.T) {
	s := &server{block: make(chan struct{})}
	sh := newShipper(t, s, 1)
	sent := 0
	for range 10 {
		if sh.Enqueue([]byte("b"), "t") {
			sent++
		}
	}
	// One batch in flight (blocked in the server), one queued; the rest dropped.
	if sh.Dropped() < 8 || sent+int(sh.Dropped()) != 10 {
		t.Errorf("sent=%d dropped=%d", sent, sh.Dropped())
	}
	close(s.block)
	if err := sh.Close(context.Background()); err != nil {
		t.Fatal(err)
	}
	if sh.Enqueue([]byte("late"), "t") {
		t.Error("enqueue after close accepted")
	}
}

func TestCloseHonoursDeadline(t *testing.T) {
	s := &server{block: make(chan struct{})}
	defer close(s.block)
	sh := newShipper(t, s, 0)
	sh.Enqueue([]byte("b"), "t")
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Millisecond)
	defer cancel()
	if err := sh.Close(ctx); err == nil {
		t.Error("Close returned before the queue drained")
	}
}
