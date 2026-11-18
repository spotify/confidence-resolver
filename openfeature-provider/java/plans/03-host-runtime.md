# 03 — Host runtime for state-to-wasm modules

Ports go commit `31cd404a` (host runtime) with the fixes from `c85edc44` (discard abandoned
suspended processes; report failed allocations). One commit of the rewrite PR.

**Status: ✅ landed — 23 tests green** (EnvelopeTest 5, ModuleTest 3, ResolverTest 10,
HostBenchmarkTest 1, TestdataTest 4), verified in the harness (the real pom, module and
fixtures, minus the old stack that still blocks compilation).

## Goal

`com.spotify.confidence.sdk.internal.host` runs one module generation on Endive 1.2.0 (spike-verified;
see `01-chicory-spike.md` for the API recipe): validates the
module, owns its capped shared memory, lazily fills a bounded instance pool, and exposes
resolve/evaluate/apply/flush/metrics/close. Port `internal/host` file by file:

| Go file | Java class | Purpose |
|---|---|---|
| `module.go` | `Module` | compile + validate: required exports, required imports, `host.memory` shared flag, optional exports (`allocated_bytes`, `metrics_snapshot`, `observe_client_resolve_latency`, `unpack_bitsets`) |
| `imports.go` | (in `Module`) | Endive validates host memories against declared imports (limits + shared) itself; keep only the export/import-set validation |
| `memory.go` | `SharedMemory` | build the shared memory, initial 1 page, max `min(MaxMemoryBytes, declared)`, default cap 64 MiB, **reserved up front** (it shows as heap) |
| `resolver.go` | `Resolver` | one generation's runtime: filter links, env host module, lazy instance pool, the call surface below; `ErrClosed` |
| `envelope.go` | `ProcessEnvelope` | decode only the envelope fields needed: result kind, suspension (process id + reads), materialization writes; the full resolve response is read in place |
| `filter.go` | `DedupeFilter`, `Filter`, `NeverSeen`, `AlwaysSeen`, `LinkedFilter` | dedupe state in the filter's own shared memory (engine-scoped, outlives generations), one filter instance per module instance, direct wasm-to-wasm link (or the spike's trampoline) |
| `race.go` / `norace.go` | — | not needed (Go race-detector annotation only) |

## Call surface (from `resolver.go`)

- `Evaluate(ctx, resolveRequest, evaluateRequests, store) -> List<ResolutionDetails>` — the
  OpenFeature path.
- `Resolve(ctx, resolveRequest, store) -> ResolveFlagsResponse` — the direct path, keeps the
  resolve token.
- Suspend/resume loop: `resolve_process_start` → `suspended` → `store.read(records)` →
  `resolve_process_resume` (may suspend again → loop). **Every non-resuming path discards the
  process** — caller cancelled, records unmarshalable, store failed — waiting for an instance
  regardless of the caller's deadline (bounded: instances are held only for module calls). A
  resume that suspends again discards the new process too.
- `ApplyFlags(ctx, request)`; `FlushLogs(ctx)`; `Warm(ctx)`; `Metrics(ctx, openMetrics)` (renders
  Prometheus/OpenMetrics text via `metrics_snapshot`; `""` when unexported);
  `AllocatedBytes(ctx)` when exported; `Close(ctx)` — final flush, `destroy_instance` each
  instance, then tear the runtime down.
- **Allocation failures are errors at every call site** — never silently "no materialization
  support" (the Go bug `c85edc44` fixed: a null slice meant no-materialization to `resume`).

## Mechanics to port faithfully

- Requests/responses through the shared memory via `alloc`/`free`; the per-instance two-slot stack
  carries call arguments and results.
- Instance pool: a bounded idle queue (Go uses a channel of instances); callers wait bounded by
  their context; lazy creation up to `Instances`; creation and shutdown guarded by one lock.
- `write_logs(i64)`: copy the `WriteLogsPayload` parts out of wasm memory into **one owned byte
  buffer**, then ack re-entrantly via `ack_logs` (or the spike's deferred-ack design), then hand
  the buffer to the configured callback. The callback must not block or call back into the
  resolver — enqueue only.
- `env.current_time` epoch millis; `env.random_seed` from `SecureRandom`.
- Host import contract lives in the generation's runtime; filter instances link into each module
  instance.
- Store boundary (`host.MaterializationStore`): `read(ctx, List<Record>) -> List<Record>` /
  `write(ctx, List<Record>)` on the **wasm record type** — the public Java store is adapted to it
  in phase 7. Store writes from the module go out asynchronously (don't hold the instance).

## Tests (port `resolver_test.go`, `module_test.go`, `envelope_test.go`, `bench_test.go`)

- Resolve against `basic.wasm` `flags/plain` for all value kinds; nested struct values.
- Sticky path: suspension → store read → resume; and a store that returns nothing (fallback
  rule). Discard on: caller cancel mid-suspension, store failure, garbage records. No leaked
  processes (repeat and watch `allocated_bytes`).
- Concurrency: N threads evaluating on a pool of 2 without trap; pool exhaustion bounded by
  context.
- Memory cap: module declaring 4 GiB max gets a 64 MiB cap; cap honored on growth.
- Filters: `seen` 0-then-1 for a repeated hash; `NeverSeen`/`AlwaysSeen` constants; filter memory
  survives a resolver close (engine scope).
- `flush_logs` → `write_logs` payload received, acked, callback non-blocking; final flush on
  close; `ErrClosed` after close.
- A benchmark of the evaluate path (informs phase 7's README perf claims).

## Done when

All the above run against `basic.wasm` with Endive 1.2.0 (both engines), and `make test`
in `openfeature-provider/java` passes (verified in the harness until the old stack's removal
makes the module itself compile).

## Deltas from the Go reference (as landed)

- `MemoryLimits` comes from `run.endive.wasm.types`; the shared memory is a host-built
  `ByteArrayMemory` — no shared-memory wasm module needed, as wazero forced on Go. A cap below
  the module's declared minimum fails at build; an over-tight cap that the module can't even
  initialize within makes it trap while instantiating, mapped to `InvalidModuleException` at the
  first instance (fail fast, never mid-resolve).
- Endive's shared memory grows on demand, so a 4 MiB resume record fits a 64 MiB cap — unlike
  wazero, where the Go test's huge record fails at the default cap. The abandoned-process test
  pins the same behavior with a 1 MiB cap: the allocation failure surfaces as
  `AllocationException` and discards the process (exactly one discard, no leak).
- The envelope is the generated protobuf parse of `ResolveProcessResponse` — one parse instead
  of Go's hand-rolled partial decode; the raw envelope slice is kept for `evaluate`, which reads
  the resolve result in place.
- `MaterializationStore` is a top-level interface (as in Go); `Resolver` implements
  `AutoCloseable` (idempotent close, final flush on the first instance).
- Filter constants (`neverSeen`/`alwaysSeen`) are plain host functions; `DedupeFilter`
  instantiates the embedded filter module per main instance and links it
  (`ImportFunction.exportAsImport`); constant links have no instance, and the resolver closes
  filter instances with their main instance.
- Instances are not thread-safe in Endive; the pool (an `ArrayBlockingQueue` + creation lock,
  mirroring Go's channel + mutex) serializes per instance.
- `HostBenchmarkTest` reports ~99 µs per plain evaluate and ~109 µs per sticky-suspend evaluate
  (single-threaded, four instances, macOS/arm64, runtime-compiler engine) — a full
  resolve-process per evaluation; the spike's tight repeated-evaluate loop measured ~22 µs,
  the number to compare with wazero's JIT.
