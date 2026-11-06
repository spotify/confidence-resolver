package compiled_module

import (
	"encoding/binary"
	"fmt"
)

// wasmImportKind mirrors the WebAssembly binary-format import kinds.
type wasmImportKind byte

const (
	wasmImportFunc   wasmImportKind = 0x00
	wasmImportTable                 = 0x01
	wasmImportMemory                = 0x02
	wasmImportGlobal                = 0x03
)

// wasmImport describes one entry of a WASM import section.
type wasmImport struct {
	Module    string
	Name      string
	Kind      wasmImportKind
	MemLimits *memoryLimits // set when Kind == wasmImportMemory
}

// memoryLimits describes a memory import's declared limits, in pages.
type memoryLimits struct {
	Initial uint32
	Maximum uint32
	HasMax  bool
	Shared  bool
}

// wasmInfo is the distilled result of parsing a WASM binary's import, export
// and custom sections.
type wasmInfo struct {
	Imports          []wasmImport
	Exports          map[string]bool
	HostMemoryLimits memoryLimits
	MetadataPayload  []byte // payload of the confidence.metadata custom section
}

// parseWasmInfo walks a WASM binary and extracts the host-relevant sections.
// It is deliberately defensive: any truncated or malformed binary yields an
// error rather than a panic.
func parseWasmInfo(wasm []byte) (*wasmInfo, error) {
	if len(wasm) < 8 {
		return nil, fmt.Errorf("wasm binary too short")
	}
	if binary.LittleEndian.Uint32(wasm[0:4]) != 0x6d736100 || binary.LittleEndian.Uint32(wasm[4:8]) != 1 {
		return nil, fmt.Errorf("not a WebAssembly binary (bad magic/version)")
	}
	r := &binReader{data: wasm, pos: 8}
	info := &wasmInfo{Exports: map[string]bool{}}
	sawImports := false

	for r.pos < len(r.data) {
		sectionID, err := r.readByte()
		if err != nil {
			return nil, err
		}
		size, err := r.readUleb()
		if err != nil {
			return nil, err
		}
		body, err := r.readBytes(size)
		if err != nil {
			return nil, err
		}
		section := &binReader{data: body}

		switch sectionID {
		case 0: // custom
			name, err := section.readName()
			if err != nil {
				return nil, fmt.Errorf("custom section name: %w", err)
			}
			if name == metadataSectionName && len(info.MetadataPayload) == 0 {
				info.MetadataPayload = section.data[section.pos:]
			}
		case 2: // import
			sawImports = true
			count, err := section.readUleb()
			if err != nil {
				return nil, err
			}
			for i := uint64(0); i < count; i++ {
				imp, err := section.readImport()
				if err != nil {
					return nil, err
				}
				info.Imports = append(info.Imports, imp)
				if imp.Kind == wasmImportMemory && imp.Module == memoryImportModule && imp.Name == memoryImportField {
					limits := imp.MemLimits
					if limits == nil {
						return nil, fmt.Errorf("missing limits on %s.%s import", memoryImportModule, memoryImportField)
					}
					info.HostMemoryLimits = *limits
				}
			}
			if err := section.expectEnd(); err != nil {
				return nil, err
			}
		case 7: // export
			count, err := section.readUleb()
			if err != nil {
				return nil, err
			}
			for i := uint64(0); i < count; i++ {
				name, err := section.readName()
				if err != nil {
					return nil, err
				}
				kind, err := section.readByte()
				if err != nil {
					return nil, err
				}
				if _, err := section.readUleb(); err != nil { // item index
					return nil, err
				}
				switch kind {
				case 0x00: // func
					info.Exports[name] = true
				}
			}
			if err := section.expectEnd(); err != nil {
				return nil, err
			}
		}
	}
	if !sawImports {
		return nil, fmt.Errorf("module has no import section")
	}
	if !info.HostMemoryLimits.Shared || info.HostMemoryLimits.Initial == 0 {
		return nil, fmt.Errorf("module does not import a shared %s.%s memory",
			memoryImportModule, memoryImportField)
	}
	return info, nil
}

// binReader is a bounds-checked cursor over a byte slice.
type binReader struct {
	data []byte
	pos  int
}

func (r *binReader) readByte() (byte, error) {
	if r.pos >= len(r.data) {
		return 0, fmt.Errorf("truncated wasm binary")
	}
	b := r.data[r.pos]
	r.pos++
	return b, nil
}

func (r *binReader) readBytes(n uint64) ([]byte, error) {
	if n > uint64(len(r.data)-r.pos) {
		return nil, fmt.Errorf("truncated wasm binary")
	}
	out := r.data[r.pos : r.pos+int(n)]
	r.pos += int(n)
	return out, nil
}

func (r *binReader) readUleb() (uint64, error) {
	var result uint64
	for shift := uint(0); ; shift += 7 {
		if shift >= 64 {
			return 0, fmt.Errorf("uleb128 overflow")
		}
		b, err := r.readByte()
		if err != nil {
			return 0, err
		}
		result |= uint64(b&0x7f) << shift
		if b&0x80 == 0 {
			return result, nil
		}
	}
}

func (r *binReader) readName() (string, error) {
	n, err := r.readUleb()
	if err != nil {
		return "", err
	}
	raw, err := r.readBytes(n)
	if err != nil {
		return "", err
	}
	return string(raw), nil
}

func (r *binReader) readImport() (wasmImport, error) {
	module, err := r.readName()
	if err != nil {
		return wasmImport{}, err
	}
	name, err := r.readName()
	if err != nil {
		return wasmImport{}, err
	}
	kind, err := r.readByte()
	if err != nil {
		return wasmImport{}, err
	}
	imp := wasmImport{Module: module, Name: name, Kind: wasmImportKind(kind)}
	switch kind {
	case 0x00: // func: type index
		if _, err := r.readUleb(); err != nil {
			return wasmImport{}, err
		}
	case 0x01: // table: element type + limits
		if _, err := r.readByte(); err != nil {
			return wasmImport{}, err
		}
		if err := r.skipLimits(); err != nil {
			return wasmImport{}, err
		}
	case 0x02: // memory: limits (read eagerly so the stream stays aligned)
		limits, err := r.readMemoryLimits()
		if err != nil {
			return wasmImport{}, err
		}
		imp.MemLimits = &limits
	case 0x03: // global: value type + mutability
		if _, err := r.readByte(); err != nil {
			return wasmImport{}, err
		}
		if _, err := r.readByte(); err != nil {
			return wasmImport{}, err
		}
	default:
		return wasmImport{}, fmt.Errorf("unknown import kind %d", kind)
	}
	return imp, nil
}

// readMemoryLimits parses a memory-type limits entry (flags byte + min [+ max]).
func (r *binReader) readMemoryLimits() (memoryLimits, error) {
	var limits memoryLimits
	flags, err := r.readByte()
	if err != nil {
		return limits, err
	}
	if flags&0x04 != 0 {
		return limits, fmt.Errorf("64-bit memory imports are not supported")
	}
	min, err := r.readUleb()
	if err != nil {
		return limits, err
	}
	if min > 65536 {
		return limits, fmt.Errorf("memory minimum of %d pages is too large", min)
	}
	limits.Initial = uint32(min)
	limits.HasMax = flags&0x01 != 0
	limits.Shared = flags&0x02 != 0
	if limits.HasMax {
		max, err := r.readUleb()
		if err != nil {
			return limits, err
		}
		if max > 65536 || max < min {
			return limits, fmt.Errorf("invalid memory maximum %d pages", max)
		}
		limits.Maximum = uint32(max)
	}
	return limits, nil
}

// skipLimits consumes a table-type limits entry (flags byte + min [+ max]).
func (r *binReader) skipLimits() error {
	flags, err := r.readByte()
	if err != nil {
		return err
	}
	if _, err := r.readUleb(); err != nil {
		return err
	}
	if flags&0x01 != 0 {
		_, err := r.readUleb()
		return err
	}
	return nil
}

func (r *binReader) expectEnd() error {
	if r.pos != len(r.data) {
		return fmt.Errorf("trailing bytes in section")
	}
	return nil
}
