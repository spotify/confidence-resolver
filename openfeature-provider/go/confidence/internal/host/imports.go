package host

import (
	"encoding/binary"
	"errors"
)

// memoryImportShared reports whether wasm's host.memory import is declared shared. wazero
// validates the module but doesn't expose this flag, and it instantiates an unshared import
// against our shared memory, so we read it from the import section ourselves.
func memoryImportShared(wasm []byte) (bool, error) {
	r := reader{b: wasm}
	if len(wasm) < 8 {
		return false, errors.New("module too short")
	}
	r.b = r.b[8:] // magic and version; wazero has already validated them
	for len(r.b) > 0 {
		id := r.byte()
		body := r.bytes(int(r.uleb()))
		if r.err != nil {
			return false, r.err
		}
		if id != 2 {
			continue
		}
		s := reader{b: body}
		for n := s.uleb(); n > 0 && s.err == nil; n-- {
			mod, name := string(s.bytes(int(s.uleb()))), string(s.bytes(int(s.uleb())))
			switch kind := s.byte(); kind {
			case 0x00: // func
				s.uleb()
			case 0x01: // table
				s.byte()
				s.limits()
			case 0x02: // memory
				flags := s.limits()
				if mod == "host" && name == "memory" {
					return flags&0x02 != 0, s.err
				}
			case 0x03: // global
				s.byte()
				s.byte()
			case 0x04: // tag
				s.byte()
				s.uleb()
			default:
				return false, errors.New("unknown import kind")
			}
		}
		return false, errors.Join(s.err, errors.New("no host.memory import"))
	}
	return false, errors.New("no import section")
}

type reader struct {
	b   []byte
	err error
}

func (r *reader) byte() byte {
	if len(r.b) == 0 {
		r.err = errors.New("truncated module")
		return 0
	}
	v := r.b[0]
	r.b = r.b[1:]
	return v
}

func (r *reader) uleb() uint64 {
	v, n := binary.Uvarint(r.b)
	if n <= 0 {
		r.err = errors.New("bad LEB128")
		return 0
	}
	r.b = r.b[n:]
	return v
}

func (r *reader) bytes(n int) []byte {
	if n < 0 || n > len(r.b) {
		r.err = errors.New("truncated module")
		return nil
	}
	v := r.b[:n]
	r.b = r.b[n:]
	return v
}

// limits reads memory or table limits and returns their flags byte.
func (r *reader) limits() byte {
	flags := r.byte()
	r.uleb()
	if flags&0x01 != 0 {
		r.uleb()
	}
	return flags
}
