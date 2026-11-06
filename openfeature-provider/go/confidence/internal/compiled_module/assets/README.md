`dedupe-filter-threaded.wasm` is copied without modification from
`state-to-wasm/dedupe-filter/dedupe-filter-threaded.wasm` (built with Zig 0.16.0).
It exports `seen(i64) -> i32` and imports shared `env.memory`.

To update from a sibling checkout, run from the repository root:

```sh
cp ../state-to-wasm/dedupe-filter/dedupe-filter-threaded.wasm \
  openfeature-provider/go/confidence/internal/compiled_module/assets/
```

Each filter owns fresh zeroed memory. Preserve it across client-module updates;
never reuse memory from an incompatible filter layout.
