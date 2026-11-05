package confidence

import (
	"sync"
	"time"
)

// seenFilter defaults.
const (
	seenFilterTTL     = 10 * time.Minute
	seenFilterMaxKeys = 65536
)

// seenFilterEntry is one ring slot: a fingerprint and when its TTL ends.
type seenFilterEntry struct {
	fingerprint int64
	expires     time.Time
}

// seenFilter is the host-side de-duplication set behind the module's
// apply_filter.seen and schema_filter.seen imports: it remembers reported
// fingerprints for a TTL window so repeated identical assignments or schema
// payloads are logged once instead of on every resolve.
//
// Every operation is O(1). Entries live in an insertion-ordered ring of
// maxKeys slots; expiry is checked lazily on lookup, and when the ring is
// full the oldest slot is evicted to admit the newcomer. Under sustained
// unique-key pressure (the ring wrapping within the TTL) this degrades to a
// bounded FIFO window — the same steady state the previous map-plus-scan
// implementation reached, but without its full-map scans per insert, which
// serialized all callers and capped throughput at a few thousand resolves
// per second.
//
// One nuance: a fingerprint that expires and is re-recorded keeps its old
// ring slot, so under ring pressure it can be evicted before its new TTL
// lapses and reported again. De-duplication is best-effort exposure
// suppression, so one extra event in that corner is acceptable for the O(1)
// guarantee.
//
// When disabled (disabled), every lookup reports "not seen", which turns
// de-duplication off: correct, but it sends more exposure events.
type seenFilter struct {
	mu       sync.Mutex
	disabled bool
	ttl      time.Duration
	maxKeys  int // 0 means seenFilterMaxKeys; smaller values are for tests
	now      func() time.Time

	ring  []seenFilterEntry // fixed capacity, insertion-ordered
	head  int               // oldest slot; evicted first
	size  int               // occupied slots (live or expired)
	index map[int64]int     // fingerprint -> ring slot
}

func newSeenFilter(disabled bool) *seenFilter {
	return &seenFilter{
		disabled: disabled,
		ttl:      seenFilterTTL,
		now:      time.Now,
	}
}

// Seen reports whether the fingerprint was already reported, recording it
// when not.
func (f *seenFilter) Seen(fingerprint int64) bool {
	if f == nil || f.disabled {
		return false
	}

	f.mu.Lock()
	defer f.mu.Unlock()

	now := f.now()
	if f.ring == nil {
		capacity := f.maxKeys
		if capacity <= 0 {
			capacity = seenFilterMaxKeys
		}
		f.ring = make([]seenFilterEntry, capacity)
		f.index = make(map[int64]int, capacity)
	}

	if slot, ok := f.index[fingerprint]; ok {
		entry := f.ring[slot]
		if now.Before(entry.expires) {
			return true
		}
		// Expired: re-record in place; the slot keeps its position.
		f.ring[slot] = seenFilterEntry{fingerprint: fingerprint, expires: now.Add(f.ttl)}
		return false
	}

	// Not present: admit it, evicting the oldest slot when the ring is
	// full (whether that slot is live or expired).
	if f.size < len(f.ring) {
		f.ring[f.size] = seenFilterEntry{fingerprint: fingerprint, expires: now.Add(f.ttl)}
		f.index[fingerprint] = f.size
		f.size++
		return false
	}

	oldest := f.ring[f.head]
	delete(f.index, oldest.fingerprint)
	f.ring[f.head] = seenFilterEntry{fingerprint: fingerprint, expires: now.Add(f.ttl)}
	f.index[fingerprint] = f.head
	f.head = (f.head + 1) % len(f.ring)
	return false
}
