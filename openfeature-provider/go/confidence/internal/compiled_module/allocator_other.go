//go:build !unix

package compiled_module

// reserveRegion is unavailable on this platform; the allocator falls back to
// an eagerly committed region (see commitRegion).
func reserveRegion(capacity, max uint64) (region []byte, free func(), err error) {
	return nil, nil, errUnsupportedReservation
}
