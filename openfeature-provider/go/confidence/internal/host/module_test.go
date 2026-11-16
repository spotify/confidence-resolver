package host

import (
	"context"
	"errors"
	"os"
	"strings"
	"testing"
)

func fixtureB(t testing.TB, name string) []byte { return fixture(t, name) }

func fixture(t testing.TB, name string) []byte {
	t.Helper()
	b, err := os.ReadFile("../../../../testdata/modules/" + name)
	if err != nil {
		t.Fatalf("read fixture: %v", err)
	}
	return b
}

func engines(t *testing.T, f func(t *testing.T, rt *Runtime)) {
	for _, interp := range []bool{false, true} {
		name := "compiler"
		if interp {
			name = "interpreter"
		}
		t.Run(name, func(t *testing.T) {
			ctx := context.Background()
			rt := NewRuntime(ctx, interp)
			t.Cleanup(func() { _ = rt.Close(ctx) })
			f(t, rt)
		})
	}
}

func TestCompileFixture(t *testing.T) {
	engines(t, func(t *testing.T, rt *Runtime) {
		m, err := rt.Compile(context.Background(), fixture(t, "basic.wasm"))
		if err != nil {
			t.Fatal(err)
		}
		if m.MinPages == 0 || m.MaxPages < m.MinPages {
			t.Errorf("limits min=%d max=%d", m.MinPages, m.MaxPages)
		}
		for _, name := range []string{"allocated_bytes", "metrics_snapshot", "observe_client_resolve_latency", "unpack_bitsets"} {
			if !m.Optional[name] {
				t.Errorf("optional export %s not detected", name)
			}
		}
	})
}

func TestCompileRejects(t *testing.T) {
	cases := map[string]struct {
		wasm []byte
		want string
	}{
		"garbage":           {[]byte("not wasm"), ""},
		"no memory import":  {emptyModule(), "expected one imported memory"},
		"wrong memory name": {memoryImportModule("env", "memory", true), "host.memory"},
		"missing exports":   {memoryImportModule("host", "memory", true), "missing export"},
	}
	ctx := context.Background()
	rt := NewRuntime(ctx, false)
	defer rt.Close(ctx)
	for name, tc := range cases {
		t.Run(name, func(t *testing.T) {
			_, err := rt.Compile(ctx, tc.wasm)
			if !errors.Is(err, ErrInvalidModule) {
				t.Fatalf("err = %v, want ErrInvalidModule", err)
			}
			if !strings.Contains(err.Error(), tc.want) {
				t.Errorf("err = %v, want it to mention %q", err, tc.want)
			}
		})
	}
}

func emptyModule() []byte {
	return []byte{0, 'a', 's', 'm', 1, 0, 0, 0}
}

// memoryImportModule imports one memory (min 1, max 2) and nothing else.
func memoryImportModule(mod, name string, shared bool) []byte {
	flags := byte(0x01)
	if shared {
		flags = 0x03
	}
	var imp []byte
	imp = append(imp, 1) // one import
	imp = append(imp, byte(len(mod)))
	imp = append(imp, mod...)
	imp = append(imp, byte(len(name)))
	imp = append(imp, name...)
	imp = append(imp, 0x02, flags, 1, 2)
	b := emptyModule()
	b = append(b, 2, byte(len(imp)))
	return append(b, imp...)
}
