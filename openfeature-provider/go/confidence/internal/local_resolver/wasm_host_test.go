package local_resolver

import (
	"bytes"
	"context"
	"testing"
)

func TestHostCallbacksDoNotUseClosedSiblingResolver(t *testing.T) {
	for _, interpreter := range []bool{false, true} {
		name := "jit"
		if interpreter {
			name = "interpreter"
		}
		t.Run(name, func(t *testing.T) {
			factory := NewWasmResolverFactory(NoOpLogSink, interpreter)
			t.Cleanup(func() { _ = factory.Close(context.Background()) })
			closed := factory.New().(*WasmResolver)
			for _, name := range []string{"wasm_msg_alloc", "wasm_msg_free"} {
				closed.exportedFunction(name)
			}
			if err := closed.Close(context.Background()); err != nil {
				t.Fatal(err)
			}
			live := factory.New().(*WasmResolver)
			t.Cleanup(func() { _ = live.Close(context.Background()) })
			for name, ctx := range map[string]context.Context{
				"no_resolver":       context.Background(),
				"matching_resolver": context.WithValue(context.Background(), wasmResolverContextKey{}, live),
				"closed_sibling":    context.WithValue(context.Background(), wasmResolverContextKey{}, closed),
			} {
				t.Run(name, func(t *testing.T) {
					for _, size := range []int{0, 1, 4096, 65537} {
						want := bytes.Repeat([]byte{0xa7}, size)
						ptr := transfer(ctx, live.instance, want)
						got := consume(ctx, live.instance, ptr)
						if !bytes.Equal(got, want) {
							t.Fatalf("memory roundtrip changed %d-byte payload", size)
						}
					}
				})
			}
		})
	}
}
