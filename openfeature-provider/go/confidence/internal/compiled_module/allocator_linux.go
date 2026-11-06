//go:build linux

package compiled_module

import "syscall"

// linuxNoReserve returns the Linux flag that keeps a private anonymous
// reservation from counting against the commit limit.
func linuxNoReserve() int {
	return syscall.MAP_NORESERVE
}
