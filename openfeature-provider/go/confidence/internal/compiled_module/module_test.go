package compiled_module

import (
	"bytes"
	"crypto/sha256"
	"os"
	"path/filepath"
	"testing"

	"github.com/spotify/confidence-resolver/openfeature-provider/go/confidence/internal/proto/flagswasm"
	"google.golang.org/protobuf/proto"
)

// tutorialSecret is the credential baked into testdata/tutorial_client_module.wasm
// (compiled from the resolver_state.pb tutorial fixture state). It is not a
// live environment secret.
const tutorialSecret = "mkjJruAATQWjeY7foFIWfVAcBWnci2YF"

// e2eSecretA and e2eSecretB are the two credentials baked into
// testdata/e2e_client_module.wasm (compiled from the e2e fixture state).
const (
	e2eSecretA = "RxDVTrXvc6op1XxiQ4OaR31dKbJ39aYV"
	e2eSecretB = "ti5Sipq5EluCYRG7I5cdbpWC3xq7JTWv"
)

func loadFixture(t *testing.T, name string) []byte {
	t.Helper()
	data, err := os.ReadFile(filepath.Join("..", "..", "testdata", name))
	if err != nil {
		t.Fatalf("failed to load fixture %s: %v", name, err)
	}
	return data
}

func TestLoadModule(t *testing.T) {
	wasm := loadFixture(t, "tutorial_client_module.wasm")

	mod, err := LoadModule(wasm, tutorialSecret)
	if err != nil {
		t.Fatalf("LoadModule: %v", err)
	}
	if mod.Metadata.GetSchemaVersion() != 1 {
		t.Errorf("schema version = %d, want 1", mod.Metadata.GetSchemaVersion())
	}
	hash := sha256.Sum256([]byte(tutorialSecret))
	if !containsBytes(mod.Metadata.GetClientSecretSha256(), hash[:]) {
		t.Errorf("metadata does not list the tutorial secret hash")
	}
	for _, name := range requiredExports {
		if !mod.HasExport(name) {
			t.Errorf("fixture lacks required export %q", name)
		}
	}
	// The current producer does not emit the optional exports; absence must
	// be representable.
	if mod.HasExport("metrics_snapshot") {
		t.Logf("fixture has metrics_snapshot; optional export present")
	}
}

func TestLoadModuleRejectsWrongSecret(t *testing.T) {
	wasm := loadFixture(t, "tutorial_client_module.wasm")
	if _, err := LoadModule(wasm, "definitely-not-the-tutorial-secret"); err == nil {
		t.Fatal("expected module for another credential to be rejected")
	}
}

func TestLoadModuleRejectsGarbage(t *testing.T) {
	cases := [][]byte{
		nil,
		[]byte("not wasm at all"),
		{0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00}, // header only
		bytes.Repeat([]byte{0x02}, 64),                   // import-section-shaped noise
	}
	for i, b := range cases {
		if _, err := LoadModule(b, tutorialSecret); err == nil {
			t.Errorf("case %d: expected garbage to be rejected", i)
		}
	}
}

// TestLoadModuleRejectsNonSharedMemoryModule loads a module compiled without
// the threaded runtime: it defines its own memory instead of importing a
// shared host.memory, which the client-module contract forbids.
func TestLoadModuleRejectsNonSharedMemoryModule(t *testing.T) {
	wasm, err := os.ReadFile(filepath.Join("..", "..", "testdata", "tutorial_module_singlethreaded.wasm"))
	if err != nil {
		t.Skipf("single-threaded fixture not built: %v", err)
	}
	if _, err := LoadModule(wasm, tutorialSecret); err == nil {
		t.Fatal("expected non-shared module to be rejected")
	}
}

func TestMetadataSectionParsing(t *testing.T) {
	// The metadata payload is read from the raw bytes without instantiating
	// anything; assert it is a valid ResolverMetadata.
	wasm := loadFixture(t, "tutorial_client_module.wasm")
	info, err := parseWasmInfo(wasm)
	if err != nil {
		t.Fatalf("parseWasmInfo: %v", err)
	}
	if len(info.MetadataPayload) == 0 {
		t.Fatal("no metadata payload")
	}
	meta := &flagswasm.ResolverMetadata{}
	if err := proto.Unmarshal(info.MetadataPayload, meta); err != nil {
		t.Fatalf("metadata does not decode: %v", err)
	}
	if meta.GetCompilerVersion() == "" {
		t.Error("compiler version missing")
	}
}
