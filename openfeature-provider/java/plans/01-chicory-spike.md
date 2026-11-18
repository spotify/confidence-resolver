# 01 — Chicory spike: can the JVM run threaded flag modules?

**Timeboxed gate. No PR; a decision recorded at the bottom of this file.** Everything after this
phase assumes its outcome.

## Why

The Go host runs the *threaded* state-to-wasm variant: modules import a **shared** `host.memory`,
use wasm **atomics** on it, and run as a pool of instances sharing that one memory. The current
Java provider's Chicory 1.4.0 embeds a single-instance rust-guest module with no shared memory and
no wasm-to-wasm linking — nothing in the current code proves Chicory can do what the new host
needs. `openfeature-provider/testdata/README.md` records the modules are built `--threads -E`.

Concrete requirements, from `wasm-objdump` on `openfeature-provider/testdata/modules/basic.wasm`
and the Go host (`internal/host/{module,resolver,filter}.go` on `origin/go-provider`):

1. **Runtime compilation** of arbitrary bytes (CDN-fetched, not build-time AOT): compile a
   `Module` from a byte array, instantiate, re-instantiate for the pool.
2. **Host-provided shared memory**: create one shared `Memory` (initial 1 page, max capped at 64
   MiB — the module declares max 4 GiB; check Chicory's strictness about declared-max vs
   provided-max mismatches) and supply it as the `host.memory` import to many instances.
3. **Threads-proposal opcodes**: shared-memory atomic RMW/load/store must execute correctly.
4. **Wasm-to-wasm linking**: instantiate the dedupe filter module against the *filter's* shared
   memory and link its exported `seen(i64) -> i32` into the main module's `apply_filter.seen` /
   `schema_filter.seen` imports (directly if Chicory supports instance-to-instance imports, else
   via a host trampoline — measure the trampoline cost before accepting it).
5. **Host imports**: `env.write_logs(i64)`, `env.current_time() -> i64`, `env.random_seed() -> i64`.
6. **Re-entrancy**: call the module's `ack_logs` export from inside the `write_logs` host import
   (Go acks re-entrantly). If Chicory forbids it, design the deferred-ack alternative and record
   it here.
7. **Concurrent instances**: several instances on one shared memory called from different Java
   threads. Note any Chicory caveats about memory-visibility or lock-free correctness (wasm
   atomics should make it correct; verify).
8. **Error surface**: how Chicory reports traps (exception types) so evaluation errors map
   cleanly, and that instance teardown (`destroy_instance` + free the runtime) doesn't leak.

## What to build (throwaway)

A `cmdline`-style test (or `src/test/java` scratch, not committed) that, using `basic.wasm` and
`../testdata/states/basic.json`:

- generates the protobuf classes from `../proto` (phase 2's pom wiring — do it early, informally),
- fetches nothing — loads the module from disk,
- builds a `ResolveFlagsRequest` (targeting key + the `flags/plain` and `flags/sticky` flags from
  the state), writes it through `alloc`, calls `resolve_process_start`, decodes the envelope,
  exercises a suspension against a stub store and `resolve_process_resume`,
- calls `evaluate` with an `EvaluateRequest` (default value + schema) and decodes
  `ResolutionDetails`,
- links both filters and observes `seen` returning 0 then 1 for a repeated hash,
- runs `flush_logs` and receives the payload in `write_logs`, acks, calls `destroy_instance`.

## Acceptance criteria

- All of the above runs on `basic.wasm` on the chosen Chicory version, on macOS (dev) and
  linux/amd64 + linux/arm64 (CI images).
- Instance pool of N ≥ 2 threads resolving concurrently for a minute without trap or leak
  (`allocated_bytes` stable after `free`).
- Rough latency shape recorded (not a gate, just to know): the Go JIT path is ~12 µs/eval; the
  interpreter ~590 µs. Whatever Chicory's engine costs, it should be closer to the former.

## Outcome

Record here: Chicory version chosen (or runtime alternative), any ABI workarounds (host
trampoline, deferred ack, memory-max handling), and a one-paragraph verdict per acceptance
criterion. If something fails, escalate with the evidence before starting phase 3.

- **Status:** ☐ pending
- **Chicory version:** _tbd_
- **Verdict:** _tbd_
