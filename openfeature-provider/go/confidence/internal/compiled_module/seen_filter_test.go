package compiled_module

import (
	"fmt"
	"sync"
	"sync/atomic"
	"testing"
)

func testSeenFilter(t *testing.T, interpreter bool) *SeenFilter {
	t.Helper()
	f := NewSeenFilter(false, 8)
	if err := f.Init(interpreter); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(f.Close)
	return f
}

func TestSeenFilter(t *testing.T) {
	for _, interpreter := range []bool{false, true} {
		t.Run(fmt.Sprintf("interpreter=%v", interpreter), func(t *testing.T) {
			f := testSeenFilter(t, interpreter)
			other := testSeenFilter(t, interpreter)
			for _, fp := range []int64{0, 1, -1, 1 << 62} {
				if f.Seen(fp) {
					t.Fatalf("first sighting of %d reported as seen", fp)
				}
				if !f.Seen(fp) {
					t.Fatalf("repeat sighting of %d not deduplicated", fp)
				}
				if other.Seen(fp) {
					t.Fatalf("separate filter already contains %d", fp)
				}
			}
			// Reusing the filter for another client-module generation preserves memory.
			if err := f.Init(interpreter); err != nil {
				t.Fatal(err)
			}
			if !f.Seen(1) {
				t.Fatal("initialization reset filter memory")
			}
			f.Close()
			f.Close()
			if f.Seen(1) {
				t.Fatal("closed filter should fail open")
			}
			if err := f.Init(interpreter); err == nil {
				t.Fatal("closed filter was reinitialized")
			}
		})
	}
}

func TestSeenFilterDisabled(t *testing.T) {
	f := NewSeenFilter(true, 8)
	if err := f.Init(false); err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	for i := 0; i < 3; i++ {
		if f.Seen(7) {
			t.Fatal("disabled filter reported a duplicate")
		}
	}
	if f.runtime != nil {
		t.Fatal("disabled filter allocated a runtime")
	}
}

func TestSeenFilterParallel(t *testing.T) {
	f := testSeenFilter(t, false)
	var wg sync.WaitGroup
	var admissions atomic.Int64
	for g := 0; g < 8; g++ {
		wg.Go(func() {
			for i := 0; i < 1000; i++ {
				if !f.Seen(int64(i)) {
					admissions.Add(1)
				}
			}
		})
	}
	wg.Wait()
	if got := admissions.Load(); got != 1000 {
		t.Fatalf("got %d admissions, want 1000", got)
	}
}
