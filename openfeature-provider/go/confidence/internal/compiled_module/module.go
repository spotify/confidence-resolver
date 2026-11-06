package compiled_module

import (
	"crypto/sha256"
	"fmt"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/flagswasm"
	"google.golang.org/protobuf/proto"
)

// Import names of the client-module contract.
const (
	memoryImportModule = "host"
	memoryImportField  = "memory"
)

// metadataSectionName is the WASM custom section holding routing metadata.
const metadataSectionName = "confidence.metadata"

// currentMetadataSchemaVersion is the metadata schema this host implements.
const currentMetadataSchemaVersion = 1

// requiredImports are imported by every client module.
var requiredImports = map[string]bool{
	"env.current_time": true,
	"env.random_seed":  true,
	"env.write_logs":   true,
}

// optionalImports may be pruned by the producer; both de-duplication filters
// are bound when present.
var optionalImports = map[string]bool{
	"apply_filter.seen":  true,
	"schema_filter.seen": true,
}

// requiredExports must be present on a client module for this host.
var requiredExports = []string{
	"alloc",
	"free",
	"resolve_process_start",
	"resolve_process_resume",
	"resolve_process_discard",
	"apply_flags",
	"flush_logs",
	"ack_logs",
	"evaluate",
	"destroy_instance",
}

// optionalExports are used opportunistically; absence is not an error.
var optionalExports = []string{
	"metrics_snapshot",
	"allocated_bytes",
	"observe_client_resolve_latency",
}

// Module is a validated compiled client module: a WASM binary carrying one
// credential's flag rules together with its routing metadata.
type Module struct {
	Bytes    []byte
	Metadata *flagswasm.ResolverMetadata
	info     *wasmInfo
}

// LoadModule validates raw module bytes against the client-module contract
// and returns the module with its parsed metadata.
//
// It rejects modules that import anything beyond the contract's import set,
// lack any required export, declare a non-shared host.memory import, carry a
// confidence.metadata section with an unsupported schema version, or do not
// list the given client secret's hash among the credentials they accept.
func LoadModule(bytes []byte, clientSecret string) (*Module, error) {
	info, err := parseWasmInfo(bytes)
	if err != nil {
		return nil, fmt.Errorf("invalid module binary: %w", err)
	}

	sawRequired := map[string]bool{}
	for _, imp := range info.Imports {
		key := imp.Module + "." + imp.Name
		switch {
		case imp.Module == memoryImportModule && imp.Name == memoryImportField:
			if imp.Kind != wasmImportMemory {
				return nil, fmt.Errorf("host.memory import is not a memory")
			}
		case requiredImports[key]:
			sawRequired[key] = true
		case optionalImports[key]:
			// bound when present
		default:
			return nil, fmt.Errorf("module imports %q which is not part of the client-module contract", key)
		}
	}
	for key := range requiredImports {
		if !sawRequired[key] {
			return nil, fmt.Errorf("module does not import %q", key)
		}
	}

	for _, name := range requiredExports {
		if !info.Exports[name] {
			return nil, fmt.Errorf("module does not export %q", name)
		}
	}

	metadata := &flagswasm.ResolverMetadata{}
	if len(info.MetadataPayload) == 0 {
		return nil, fmt.Errorf("module has no %s custom section", metadataSectionName)
	}
	if err := proto.Unmarshal(info.MetadataPayload, metadata); err != nil {
		return nil, fmt.Errorf("cannot parse %s custom section: %w", metadataSectionName, err)
	}
	if metadata.GetSchemaVersion() != currentMetadataSchemaVersion {
		return nil, fmt.Errorf("unsupported module metadata schema version %d (host implements %d)",
			metadata.GetSchemaVersion(), currentMetadataSchemaVersion)
	}

	if clientSecret != "" {
		secretHash := sha256.Sum256([]byte(clientSecret))
		if !containsBytes(metadata.GetClientSecretSha256(), secretHash[:]) {
			return nil, fmt.Errorf("module does not accept this client credential")
		}
	}

	return &Module{Bytes: bytes, Metadata: metadata, info: info}, nil
}

// Exports returns the module's exported function names.
func (m *Module) Exports() []string {
	names := make([]string, 0, len(m.info.Exports))
	for name := range m.info.Exports {
		names = append(names, name)
	}
	return names
}

// HasExport reports whether the module exports the named function.
func (m *Module) HasExport(name string) bool {
	return m.info.Exports[name]
}

// HasImport reports whether the module imports module.field.
func (m *Module) HasImport(module, field string) bool {
	for _, imp := range m.info.Imports {
		if imp.Module == module && imp.Name == field {
			return true
		}
	}
	return false
}

func containsBytes(set [][]byte, want []byte) bool {
	for _, candidate := range set {
		if string(candidate) == string(want) {
			return true
		}
	}
	return false
}
