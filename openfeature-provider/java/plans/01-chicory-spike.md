# 01 — Runtime spike: Endive runs threaded flag modules (passed)

**Risk gate for everything after this phase.** Originally framed as "can Chicory run threaded
modules"; the answer required its successor: **Endive**, the Bytecode Alliance's JVM-native
WebAssembly runtime (`run.endive:*`), a fork of Chicory by the same authors. Chicory on Maven
Central stops at 1.4.0 (May 2025); Endive 1.2.0 is the current line, with **threads/atomics,
multi-memory, GC, exception handling and tail calls completed** in both the interpreter and the
runtime compiler.

## Verdict

**PASS — run.endive 1.2.0 (runtime + compiler), the runtime compiler as the default engine.**
All 45 acceptance checks green on macOS/aarch64, linux/arm64 and linux/amd64, against
`openfeature-provider/testdata/modules/basic.wasm` and `dedupe-filter-threaded.wasm`.

## What was verified (per the acceptance criteria)

- **Runtime compilation of arbitrary bytes**: `Parser.parse(byte[])` at runtime;
  `MachineFactoryCompiler.compile(module)` compiles **once per module** and returns a
  `Function<Instance, Machine>` factory reused for every instance — exactly the
  one-compile-per-generation, N-instances pattern. Compile + first instance: ~2–17 ms.
- **Host-provided shared memory**: `new ByteArrayMemory(new MemoryLimits(1, 1024, true))`
  (1 initial page, 64 MiB cap, shared) passed as `ImportMemory("host", "memory", mem)`. Endive
  validates host memories against the import's declared limits (current ≥ initial,
  host max ≤ import max, shared flags equal) — no hand-rolled import-section parsing needed.
  A declared max of 65536 pages is treated as 32767 (JVM array ceiling), so any host cap ≤ 2 GiB
  links.
- **Threads opcodes**: shared-memory atomics execute correctly in **both engines** (resolve
  correctness identical).
- **Wasm-to-wasm linking**: each main instance instantiates its own filter instances
  (`Instance.builder(filterModule).withImportValues(... env.memory = the filter's shared memory
  ...)`) and links them via `ImportFunction.exportAsImport("apply_filter", "seen", filterInstance,
  "seen")` — a thin Java trampoline around the target instance's machine call; no serialization.
  Dedupe history in the filter memory outlives instances, as designed.
- **Host imports**: `HostFunction("env", "write_logs", FunctionType.of(...), handle)` etc.
- **Re-entrancy**: `ack_logs` called from inside the `write_logs` host import works (no deadlock,
  no stack corruption) — the Go re-entrant ack ports as-is.
- **Concurrency**: 2 instances on one shared memory, 8 Java threads, 16k evaluates — zero errors,
  `allocated_bytes` stable. **Endive instances are not thread-safe**; the host serializes calls
  per instance (an idle-instance pool with per-instance monitors, as the Go pool does).
- **Error surface**: traps are `TrapException`/`WasmEngineException`; module-rejected resolves
  arrive as the `Error` envelope (gRPC-style codes). A **memory cap too tight makes the module
  trap during its own initialization** → `UninstantiableException` at instance build: over-tight
  caps fail fast at generation build, which the engine maps to "invalid module, keep the current
  one" — no silent degradation. Mid-run allocation failure still surfaces as the null slice
  (`alloc` returns 0), the Go `ErrAlloc` path.
- **Behavior parity with the Go host**: resolve (values, variants, reasons, resolve token with
  `apply=false`), evaluate (struct merge, nested paths, unknown-flag fallback), sticky
  suspension → store read → resume (and fallback when the store answers nothing), apply-dedupe
  collapsing repeated applies (both evaluate and resolve paths; matches Go's
  `TestDedupeFilterSharedAcrossResolvers`), `flush_logs` payloads (packed-slice
  `WriteLogsPayload` → concatenated `WriteFlagLogsRequest`), Prometheus text from
  `metrics_snapshot`, `unpack_bitsets` warm, `allocated_bytes` balanced across 100 resolves.

## Latency (basic.wasm, `evaluate` path, per evaluation)

| Engine | macOS/aarch64 | linux/arm64 | linux/amd64 (emulated) |
|---|---|---|---|
| Interpreter | ~450 µs | ~380 µs | ~700 µs |
| Runtime compiler | ~24 µs | ~22 µs | ~47 µs |

The runtime compiler is ~20× the interpreter and lands within ~2× of wazero's JIT as measured on
the Go provider (~12 µs) — comfortably in "fast" territory, with dynamic module loading. The
interpreter is a viable fallback (and the only SIMD-capable engine, which flag modules don't use).
Chicory 1.4.0 AOT (the current provider's engine) is build-time only and cannot compile
CDN-fetched modules — Endive's runtime compiler is what makes per-credential modules work on the
JVM.

## Spike artifact

Throwaway, per the plan: `/tmp/s2w-spike` (session-local; `Spike.java` + the Endive wiring above,
compiled with `javac -cp "libs/*"` and run on the shared testdata). The essential API recipe and
verdict live here; phase 3 re-derives the wiring as real code. Say the word if you want the spike
source checked in under this directory as a reference.

## Decisions carried forward

- **Engine**: `MachineFactoryCompiler.compile(module)` per generation (compile once, reuse the
  factory for the pool); interpreter stays available as a fallback, mirroring Go's
  `UseWasmInterpreter` (rename to an engine option in `LocalProviderConfig`).
- **Memory**: host-built `ByteArrayMemory` shared per generation, cap default 64 MiB;
  filter memories `(8,8,true)` apply / `(2,2,true)` schema, engine-scoped, outliving swaps.
- **No `RecoveringResolver`**: traps and engine exceptions surface as evaluation errors; a
  broken generation is replaced only by a successful swap, as in Go.
- **poms** switch `com.dylibso.chicory:*` → `run.endive:{runtime,compiler}` (keep protobuf at the
  provider's pinned version; the spike ran 4.33.2 with protoc 29.3 gencode).
- Endive requires **Java 11+**; the provider's 17 baseline stays.

- **Status:** ✅ passed (no PR, by design)
- **Runtime:** run.endive:runtime + run.endive:compiler **1.2.0**
- **Verdict:** proceed to phase 2; no fallback runtime needed
