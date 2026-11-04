package compiled_module

import (
	"sync"

	"github.com/tetratelabs/wazero/experimental"
)

// fallbackMemoryCeiling bounds the eagerly committed fallback region on
// platforms without a reservation primitive. Modules cannot grow past it;
// grow failures surface as resolve errors, the same as real exhaustion.
const fallbackMemoryCeiling = 64 << 20 // 64 MiB

// lazyMemoryAllocator backs wazero linear memories with reserved-but-untouched
// regions so a module whose declared maximum is large (the compiler's default
// is 65536 pages = 4 GiB) costs virtual address space, not physical memory.
//
// wazero's default shared-memory path allocates make([]byte, min, max), which
// zeroes the full maximum: every host would commit the module's entire
// declared ceiling. The allocator instead reserves the region up front —
// required for shared semantics, since the backing address may never move —
// and pages become resident only when the module touches them.
type lazyMemoryAllocator struct {
	mu       sync.Mutex
	reserved *reservedMemory
}

// Compile-time check that the allocator satisfies the wazero hook.
var _ experimental.MemoryAllocator = &lazyMemoryAllocator{}

// Allocate implements experimental.MemoryAllocator. It cannot fail its
// signature, so on reservation failure it falls back to an eagerly committed
// region (wazero's own failure mode).
func (a *lazyMemoryAllocator) Allocate(capacity, max uint64) experimental.LinearMemory {
	if region, release, err := reserveRegion(capacity, max); err == nil {
		a.mu.Lock()
		a.reserved = &reservedMemory{region: region, release: release}
		a.mu.Unlock()
		return a.reserved
	}
	return &committedMemory{region: commitRegion(capacity, max)}
}

// Release frees the reserved region. It must run only after the wazero
// runtime is closed: the memory has to outlive every module instance,
// including instances that merely import it.
func (a *lazyMemoryAllocator) Release() {
	a.mu.Lock()
	defer a.mu.Unlock()
	if a.reserved != nil {
		a.reserved.release()
		a.reserved = nil
	}
}

// reservedMemory is a reserved region whose pages become resident on first
// touch. The backing address never changes, which is what shared memories
// require; wazero checks this at runtime.
type reservedMemory struct {
	region  []byte
	release func()
}

// Reallocate implements experimental.LinearMemory, returning the same base
// address with the requested length.
func (m *reservedMemory) Reallocate(size uint64) []byte {
	if size > uint64(len(m.region)) {
		// Growing beyond the reserved maximum fails the grow, like a real
		// out-of-memory would.
		return nil
	}
	return m.region[:size:size]
}

// Free implements experimental.LinearMemory as a no-op. wazero frees a
// linear memory when the first module instance importing it closes — for a
// pool of instances sharing one memory that is far too early. The host
// releases the region via lazyMemoryAllocator.Release once the runtime is
// gone.
func (m *reservedMemory) Free() {}

// committedMemory is the fallback for platforms without a reservation
// primitive: an eagerly allocated region bounded well below the module's
// declared maximum.
type committedMemory struct {
	region []byte
}

// commitRegion allocates the fallback region: the module's declared maximum
// if it is small, otherwise the fallback ceiling.
func commitRegion(capacity, max uint64) []byte {
	size := fallbackMemoryCeiling
	if max != 0 && max < uint64(size) {
		size = int(max)
	}
	if uint64(size) < capacity {
		size = int(capacity)
	}
	return make([]byte, size)
}

func (m *committedMemory) Reallocate(size uint64) []byte {
	if size > uint64(len(m.region)) {
		return nil
	}
	return m.region[:size:size]
}

func (m *committedMemory) Free() {}
