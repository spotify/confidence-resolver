package host

import "encoding/binary"

// sharedMemoryModule is a minimal module that defines one shared memory and exports it as
// "memory". wazero has no API for creating a memory on the host and passing it in, so we import
// this module under the name the resolver expects (e.g. "host").
func sharedMemoryModule(minPages, maxPages uint32) []byte {
	b := []byte{0, 'a', 's', 'm', 1, 0, 0, 0}
	mem := []byte{1, 0x03} // one memory: shared, with max
	mem = binary.AppendUvarint(mem, uint64(minPages))
	mem = binary.AppendUvarint(mem, uint64(maxPages))
	b = section(b, 5, mem)
	exp := append([]byte{1, 6}, "memory"...)
	exp = append(exp, 0x02, 0) // memory index 0
	return section(b, 7, exp)
}

func section(b []byte, id byte, body []byte) []byte {
	b = append(b, id)
	b = binary.AppendUvarint(b, uint64(len(body)))
	return append(b, body...)
}
