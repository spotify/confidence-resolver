//go:build !linux && unix

package compiled_module

// linuxNoReserve is zero on platforms without a no-reserve mmap flag
// (darwin overcommits anonymous mappings by default).
func linuxNoReserve() int {
	return 0
}
