# 00 — Architecture: the Java provider on flag modules

## Goal

Port the Go reference implementation (`origin/go-provider`) to Java: the provider fetches its
credential's **compiled flag module** (a WebAssembly module holding only that client's flags and
segments, built by state-to-wasm) from the module CDN, runs it locally in a JVM wasm runtime, and
swaps in new modules as flags change. The resolver logic and state format live in the module, not
in the provider.

What this replaces — the current Java provider (`com.spotify.confidence.sdk`, kept as API reference
on `next`):

| Today (reference) | On flag modules |
|---|---|
| One embedded rust-guest resolver WASM, AOT-compiled at build time | Per-credential module fetched at runtime, compiled per generation |
| Full account state pushed into the resolver via `setResolverState` | No state injection; a new module **is** the new state |
| State from `confidence-resolver-state-cdn.spotifycdn.com` (`ClientResolverState`) | Module from `confidence-resolver-module-cdn.spotifycdn.com` (`ClientModule`) |
| Flag logs over gRPC (`InternalFlagLoggerService`, `edge-grpc.spotify.com`) with CDN-routed destinations | Flag logs over HTTP to `ingestion.confidence.dev`, authenticated by the module's log token |
| Track events via a separate event-engine WASM + gRPC events service | Not supported yet (returns through the module later) |
| `RecoveringResolver` recreates a broken resolver; state is replayed into it | Generations swap; a module that fails to install never replaces the working one |

## Component map (Go → Java)

| Go (`openfeature-provider/go/confidence/`) | Java (`src/main/java/...`) | Responsibility |
|---|---|---|
| `provider.go`, `value.go`, `provider_builder.go` | `com.spotify.confidence.sdk` (public) | OpenFeature `FeatureProvider`, `Resolve`/`ApplyFlags`, config, value/schema encode+decode |
| `materialization_store.go`, `remote_…`, `unsupported_…` | same public package | Public `MaterializationStore` API; HTTP-protobuf remote store |
| `internal/host` | `com.spotify.confidence.sdk.internal.host` | Runs one module generation: validation, capped shared memory, instance pool, resolve/evaluate/apply/flush, host imports, dedupe filters |
| `internal/cdn` | `com.spotify.confidence.sdk.internal.cdn` | Fetch + decrypt the credential's module, ETags, error classification |
| `internal/ingest` | `com.spotify.confidence.sdk.internal.ingest` | Bounded async flag-log shipper to ingestion over HTTP |
| `internal/engine` | `com.spotify.confidence.sdk.internal.engine` | Module generations: poll, swap, flush timers, startup retries, close |
| `internal/s2wpb` | generated from `../proto` | state-to-wasm protobuf messages |
| `internal/testutil` | test sources | Fake module CDN + ingestion for tests |

The flat `com.spotify.confidence.sdk` package stays the public API; internals move to
`com.spotify.confidence.sdk.internal.*` subpackages. Old classes whose job disappears
(`WasmLocalResolver`, `PooledResolver`, `RecoveringResolver`, `MaterializingResolver`,
`ProviderTelemetryResolver`, `FlagsAdminStateFetcher`, `GrpcWasmFlagLogger`, `WasmEventTracker`,
`DefaultChannelFactory`, `ChannelFactory`, `GrpcUtil`, `DefaultDeadlineClientInterceptor`,
`AccountStateProvider`, …) are deleted with their features, not adapted.

## The module contract

Verified against `openfeature-provider/testdata/modules/basic.wasm` (`wasm-objdump`); the host must
provide and validate exactly this ABI (see `internal/host/module.go`, `imports.go`):

Imports the host supplies:

| Import | Signature | Notes |
|---|---|---|
| `host.memory` | shared memory, initial 1 page, declared max 64 Ki pages (4 GiB) | host-provided, capped to `MaxMemoryBytes` (default 64 MiB, reserved up front); shared flag must be validated by the host |
| `env.write_logs` | `(i64) -> ()` | receives a `WriteLogsPayload` (`repeated fixed64 parts`); host copies the concatenated bytes out and acks via `ack_logs` re-entrantly |
| `env.current_time` | `() -> i64` | epoch millis |
| `env.random_seed` | `() -> i64` | secure randomness |
| `apply_filter.seen` | `(i64) -> i32` | linked wasm-to-wasm to a dedupe-filter instance's `seen` |
| `schema_filter.seen` | `(i64) -> i32` | same |

Exports (required): `alloc`, `free`, `resolve_process_start`, `resolve_process_resume`,
`resolve_process_discard`, `apply_flags`, `flush_logs`, `ack_logs`, `destroy_instance`, `evaluate`.
Optional: `allocated_bytes`, `metrics_snapshot`, `observe_client_resolve_latency`,
`unpack_bitsets`, `memory`.

The dedupe filter is a second tiny wasm (`dedupe-filter-threaded.wasm`, ~1.9 KB, state-to-wasm
build): imports shared `env.memory` (initial 0 pages), exports `seen (i64) -> i32`. A filter's
memory is the dedup state; it is engine-scoped and outlives module swaps. Each module instance
links to filter instances of its own.

Protocols:

- **Resolve** (`ResolveFlagsRequest` → `ResolveFlagsResponse`): `resolve_process_start` returns a
  `ResolveProcessResponse` envelope — `resolved` (full response + `materialization_to_write` +
  `client_index`), `suspended` (`process_id` + `materialization_to_read`), or `error` (code follows
  gRPC status codes). On suspension: read the records from the store, `resolve_process_resume`;
  it may suspend again. Every path that doesn't resume **must** `resolve_process_discard`, waiting
  for an instance regardless of the caller's deadline (bounded: instances are only held for module
  calls). Allocation failures are errors at every call site — never "no materialization support".
- **Evaluate** (the OpenFeature typed path): one resolve process carrying
  `EvaluateRequest{flag_key, default_value, expected_schema}` per flag; the module answers with
  `ResolutionDetails{value, variant, reason, error_code, error_message, should_apply}`.
- **Apply**: `apply_flags` with an `ApplyFlagsRequest` (deferred apply, resolve token).
- **Logs**: `flush_logs` emits serialized `WriteFlagLogsRequest`s through `write_logs`; a host may
  concatenate fragments (field numbers align with the resolver message; last `log_mode` wins).
- Requests/responses cross the boundary through `alloc`/`free` in the shared memory; each instance
  has its own two-slot stack for call arguments.

## Network endpoints (as in Go)

| Concern | Endpoint | Notes |
|---|---|---|
| Module CDN | `https://confidence-resolver-module-cdn.spotifycdn.com/modules/<sha256(clientSecret) hex>.wasm.enc` | conditional GET (`If-None-Match`); artifact = `12-byte nonce ‖ AES-256-GCM ciphertext ‖ 16-byte tag`, empty associated data; plaintext = `ClientModule{module, log_token}` |
| Flag-log ingestion | `https://ingestion.confidence.dev/v1/flagLogs:write` | POST, `Content-Type: application/x-protobuf`, `Authorization: Bearer <log token>` |
| Remote materialization | `https://resolver.confidence.dev/v1/materialization:readMaterializedOperations`, `…/v1/materialization:writeMaterializedOperations` | protobuf over HTTP, `Authorization: ClientSecret <secret>`, read 2s / write 5s |

Error classification (CDN): 404 and 408/429/5xx retryable (404 = not yet provisioned); other 4xx,
failed decryption, undecodable artifacts terminal. An ETag is accepted only once the module built
from its body is installed — a module that fails to install is refetched in full.

## Lifecycle and concurrency

- **Generations.** Each installed module gets a `generation`: its own runtime, shared memory and
  instance pool. The engine holds the current generation behind an atomic reference; an evaluation
  read-locks the generation for its whole call, including materialization reads. A swap publishes
  the fully built + warmed new generation, then retires the old: unpublish, then acquire its write
  lock and never release it — in-flight evaluations drain, new callers see the retiring lock and
  reload from the reference.
- **Startup.** Retry every 1s within `InitializationTimeout` (default 30s); expiry is a
  *recoverable* timeout (provider reports ready later, polling continues). Credential rejections
  (terminal 4xx) and an invalid first module are *fatal* initialization errors.
- **Refresh.** Poll every 10s; failures keep the current module and leave the ETag unaccepted.
  Logs are flushed on a 30s timer, on swap and on close.
- **Close.** Stop loops, retire the current generation bounded by the caller's deadline; on
  expiry return the error and let cleanup finish in the background — nothing in use is freed.
  New evaluations fail with closed regardless.
- **Exposure modes.** `ExposureMode` replaces the old `enableApplyDedup` /
  `disableExposureCollection` switches: `DEDUPLICATED` (default; persistent apply-dedupe filter,
  8 pages), `ALL` (`NeverSeen`), `NONE` (suppress non-error exposure events; `AlwaysSeen`). The
  schema filter (2 pages) stays on independently. `_confidence_skip_apply` in the evaluation
  context still skips apply for a single evaluation and is stripped from **every** request.
- **Instances.** Pool bounded by `Instances` (default: available processors), lazily created,
  shared-memory per generation, each call serialized per instance (Endive instances are not
  thread-safe). No `RecoveringResolver`: module call failures surface as errors and
  the generation lives until the next successful swap.

## Dependency changes

| Dependency | Decision |
|---|---|
| Endive (was Chicory) | **run.endive:runtime + run.endive:compiler 1.2.0** — the Bytecode Alliance's JVM wasm runtime, Chicory's successor; spike-verified on threaded modules (phase 1). Runtime compiler engine (JIT-equivalent, dynamic loading); interpreter fallback. Instances are not thread-safe — the host serializes per instance |
| grpc-* , netty-shaded, guava | dropped — no gRPC anywhere in the new provider |
| protobuf-java (+ util) | stays, generated from `../proto`, shaded as today |
| openfeature sdk | stays |
| SLF4J | stays |

Shading keeps relocating protobuf (and whatever remains of the transitive set); gRPC and Guava
leave the shaded jar, which shrinks it substantially.

## Removed / changed public surface (breaking, released as `rc` from `next`)

- Removed: `ChannelFactory` and all gRPC config; `enableApplyDedup` /
  `disableExposureCollection` (→ `exposureMode`); `resolverPoolSize` (→ `instances`);
  `CONFIDENCE_*` gRPC-domain overrides; Track events (`track()`) until the module carries them;
  the gRPC remote materialization store (replaced by the HTTP one).
- Kept: constructor shape of `OpenFeatureLocalResolveProvider` (secret + key, config, store), the
  `MaterializationStore` interface and its sealed records, `FlagResolverService` (the HTTP JSON
  resolve/apply proxy), deferred apply + resolve tokens, Prometheus/OpenMetrics output.
- Changed: defaults (poll 10s, instances = processors, flush 30s); README-documented error
  taxonomy (recoverable timeout vs fatal misconfiguration) now actually enforced.

## Open questions

1. **Runtime** — resolved by the phase-1 spike: Endive 1.2.0 (see `01-chicory-spike.md`).
2. **Module metrics exposure** — the module's `metrics_snapshot` renders Prometheus/OpenMetrics
   text; expose via a provider method (the old `SnapshotConfig` placeholder suggested this).
   Match Go's shape.
3. **README version-drift cleanups** — the current README claims a 30s poll default where code
   said 15s; the rewrite documents the real defaults only.
