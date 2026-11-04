//go:build unix

package compiled_module

import (
	"syscall"
)

// reserveRegion reserves size bytes of anonymous private memory (read/write)
// without touching it. On Linux the region is additionally marked
// MAP_NORESERVE so it does not count against the commit budget.
func reserveRegion(capacity, max uint64) (region []byte, release func(), err error) {
	size := max
	if size < capacity {
		size = capacity
	}
	if size == 0 {
		size = 1
	}
	flags := syscall.MAP_ANON | syscall.MAP_PRIVATE
	// darwin has no MAP_NORESERVE constant; it ignores the reservation
	// problem entirely by overcommitting anonymous mappings.
	if noreserve := linuxNoReserve(); noreserve != 0 {
		flags |= noreserve
	}
	b, mmapErr := syscall.Mmap(-1, 0, int(size), syscall.PROT_READ|syscall.PROT_WRITE, flags)
	if mmapErr != nil {
		return nil, nil, mmapErr
	}
	return b, func() { _ = syscall.Munmap(b) }, nil
}
