package confidence

import (
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

func TestSeenFilterDeduplicatesWithinTTL(t *testing.T) {
	now := time.Unix(0, 0)
	f := newSeenFilter(false)
	f.now = func() time.Time { return now }

	if f.Seen(1) {
		t.Fatal("first sighting reported as seen")
	}
	if !f.Seen(1) {
		t.Fatal("second sighting within TTL not deduplicated")
	}

	now = now.Add(seenFilterTTL)
	if f.Seen(1) {
		t.Fatal("sighting after TTL not reported again")
	}
	if !f.Seen(1) {
		t.Fatal("re-recorded fingerprint not deduplicated")
	}
}

func TestSeenFilterDisabledReportsEverything(t *testing.T) {
	f := newSeenFilter(true)
	for i := 0; i < 3; i++ {
		if f.Seen(7) {
			t.Fatal("disabled filter reported a duplicate")
		}
	}
}

func TestSeenFilterEvictsOldestAtCapacity(t *testing.T) {
	now := time.Unix(0, 0)
	f := newSeenFilter(false)
	f.maxKeys = 4
	f.now = func() time.Time { return now }

	for fp := int64(1); fp <= 4; fp++ {
		f.Seen(fp)
	}
	// The ring is full: a fifth fingerprint evicts the oldest (1).
	f.Seen(5)
	// Seen is check-and-record: this miss proves 1 was evicted, and
	// re-admitting it evicts the next oldest (2) — a bounded FIFO window.
	if f.Seen(1) {
		t.Fatal("evicted fingerprint still reported as seen")
	}
	// Hits do not mutate: the window is now {1, 3, 4, 5}.
	for _, fp := range []int64{1, 3, 4, 5} {
		if !f.Seen(fp) {
			t.Fatalf("fingerprint %d should still be seen", fp)
		}
	}
	if len(f.index) != 4 {
		t.Fatalf("index size = %d, want the capacity 4", len(f.index))
	}
}

func TestSeenFilterParallel(t *testing.T) {
	f := newSeenFilter(false)
	var wg sync.WaitGroup
	for g := 0; g < 8; g++ {
		wg.Add(1)
		go func(g int) {
			defer wg.Done()
			for i := 0; i < 2000; i++ {
				f.Seen(int64(g*10000 + i))
			}
		}(g)
	}
	wg.Wait()
	if len(f.index) > seenFilterMaxKeys {
		t.Fatalf("index exceeded bound: %d", len(f.index))
	}
}

// BenchmarkSeenFilterUniqueKeys is the workload that made the previous
// map-plus-full-scan implementation the throughput bottleneck: every call
// a miss, the set at capacity, nothing expiring. Should stay in the
// sub-microsecond range instead of the ~0.55 ms the scans cost.
func BenchmarkSeenFilterUniqueKeys(b *testing.B) {
	f := newSeenFilter(false)
	var next int64 = 1
	b.ResetTimer()
	b.RunParallel(func(pb *testing.PB) {
		fp := atomic.AddInt64(&next, 1)
		for pb.Next() {
			fp += 1
			f.Seen(fp)
		}
	})
}
