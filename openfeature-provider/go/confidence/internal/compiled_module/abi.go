package compiled_module

import (
	"context"
	"fmt"
)

// Slice packing per the module ABI: low 32 bits pointer, high 32 bits length.
// The zero slice (0) means "absent". wazero passes and returns i64 values as
// uint64, so slices travel as uint64 throughout.
func packSlice(ptr, length uint32) uint64 {
	return uint64(ptr) | uint64(length)<<32
}

func unpackSlice(packed uint64) (ptr, length uint32) {
	return uint32(packed), uint32(packed >> 32)
}

// writeSlice allocates a block in the module's memory, copies bytes into it
// and returns the packed slice. Ownership passes to the module together with
// the call that consumes it: the module frees the request block itself.
// Must be called with the entry mutex held.
func (h *Host) writeSlice(bytes []byte) (uint64, error) {
	if len(bytes) == 0 {
		return 0, nil
	}
	res, err := h.alloc.Call(context.Background(), uint64(len(bytes)))
	if err != nil {
		return 0, fmt.Errorf("alloc(%d): %w", len(bytes), err)
	}
	ptr := uint32(res[0])
	if ptr == 0 {
		return 0, fmt.Errorf("alloc(%d) failed", len(bytes))
	}
	if !h.memory.Write(ptr, bytes) {
		return 0, fmt.Errorf("allocation of %d bytes is out of memory bounds", len(bytes))
	}
	return packSlice(ptr, uint32(len(bytes))), nil
}

// readSlice copies a slice out of module memory without freeing it.
// Must be called with the entry mutex held.
func (h *Host) readSlice(packed uint64) ([]byte, error) {
	ptr, length := unpackSlice(packed)
	if length == 0 {
		return nil, nil
	}
	data, ok := h.memory.Read(ptr, length)
	if !ok {
		return nil, fmt.Errorf("slice of %d bytes at %d is out of memory bounds", length, ptr)
	}
	out := make([]byte, length)
	copy(out, data)
	return out, nil
}

// readAndFreeSlice copies a slice out of module memory and frees the block.
// Used for responses the host fully consumes (evaluate results, snapshots).
// Must be called with the entry mutex held.
func (h *Host) readAndFreeSlice(packed uint64) ([]byte, error) {
	out, err := h.readSlice(packed)
	if err != nil {
		return nil, err
	}
	if ptr, length := unpackSlice(packed); length > 0 {
		if _, err := h.frees.Call(context.Background(), uint64(ptr)); err != nil {
			return nil, fmt.Errorf("free(%d): %w", ptr, err)
		}
	}
	return out, nil
}
