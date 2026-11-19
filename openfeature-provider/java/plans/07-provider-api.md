# 07 — The provider: public API on the engine ✅

Ports go commit `fb866c6d` (resolve with the credential's flag module), with `0ce35258`
(`_confidence_skip_apply`), `870eb772` (type-check every decoded value) and `c7f91211` (token
docs). One commit of the rewrite PR — the biggest one; reference `confidence/{provider,value}.go` on `next` (merged as #700).

## Goal

`OpenFeatureLocalResolveProvider` rebuilt on the `Engine`: the OpenFeature evaluation methods, the
direct `Resolve`/`ApplyFlags` API, the materialization stores, and the config surface. The class
name, artifact and constructor shape stay; behavior follows Go.

## Public surface

`LocalProviderConfig.Builder` (constructor args stay: clientSecret + encryptionKey, config,
store):

| Option | Replaces / notes |
|---|---|
| `materializationStore(MaterializationStore)` | as today |
| `useRemoteMaterializationStore(boolean)` | now the HTTP store |
| `instances(int)` | replaces `resolverPoolSize`; default `Runtime.availableProcessors()` |
| `maxMemoryBytes(long)` | new; default 64 MiB |
| `statePollInterval(Duration)` | replaces `CONFIDENCE_RESOLVER_POLL_INTERVAL_SECONDS` (default 10s) |
| `initializationTimeout(Duration)` | new; default 30s |
| `exposureMode(ExposureMode)` | replaces `enableApplyDedup` + `disableExposureCollection` |
| `httpClientFactory(HttpClientFactory)` | kept seam; used by CDN + ingest + remote store |
| `logger(...)` | SLF4J, as today |

Removed options: `channelFactory` (no gRPC), `enableApplyDedup`, `disableExposureCollection`,
`resolverPoolSize`, `useWasmInterpreter` (replaced by an engine option: Endive's runtime
  compiler is the default, the interpreter the fallback — phase 1's verdict).

## Evaluation path

- `Boolean/String/Integer/Double/Object`Details → one engine `evaluate` call: encode the flag
  default as a protobuf `Value` **plus** its `FlagSchema` (`OpenFeature` `Structure` defaults are
  map-like: no struct schema, merge semantics as documented today), translate the evaluation
  context (`targetingKey` → `targeting_key`), strip `_confidence_skip_apply` from **every**
  request (evaluations and `Resolve`; other values of it are ignored — `true` resolves that one
  evaluation with apply off, resolve analytics still on).
- Decode `ResolutionDetails` back to the OpenFeature type. **Type-check every decoded value,
  including values nested under maps** (`870eb772`): a mismatch anywhere in the tree →
  `TYPE_MISMATCH`-style error and the caller's default, never a silently zeroed value. Nulls in
  the flag use the default for that field.
- Nested value paths keep `FlagPath` semantics; errors map to OpenFeature `ErrorCode`s as today
  (`FLAG_NOT_FOUND` etc.).
- Fatal-startup behavior: `initialize()` throws on terminal first-fetch outcomes; on recoverable
  timeout it returns the provider `NOT_READY`→`READY` via events when the background install
  lands, as the old provider did.

## Direct resolve API

- `Resolve(evaluationContext, flagNames, apply)` → `ResolveFlagsResponse`; flag names without
  the `flags/` prefix; `apply=false` responses carry `resolve_token`.
- `ApplyFlags(ApplyFlagsRequest)` — deferred apply with the token. Document, as Go does after
  `c7f91211`: **the resolve token is neither encrypted nor authenticated** — it is the serialized
  `FlagAssigned`; a token changed in transit logs exposures that never happened. Recommend
  backend-owned authenticated encryption or a server-side reference.
- `track(...)` — throw the SDK's unsupported-operation for now; README says event tracking
  returns through the module later.

## Materialization

- Keep the public `MaterializationStore` interface and its sealed `ReadOp`/`ReadResult`/`WriteOp`
  records unchanged (users implement it). An internal adapter maps them to the host's wasm
  records. Writes stay async and idempotent by contract.
- `RemoteMaterializationStore` rewritten: protobuf over HTTP to
  `https://resolver.confidence.dev/v1/materialization:{read,write}MaterializedOperations`,
  `Authorization: ClientSecret <secret>`, read 2s / write 5s (env overrides keep their names).
- `UnsupportedMaterializationStore` stays the default: evaluations that need materializations
  fall back with the materialization-not-supported reason (keep today's behavior and
  `STICKY_RESOLVE.md`'s corrected wording).

## Kept Java-specific surface

- `FlagResolverService` (the framework-neutral JSON `/v1/flags:resolve` / `:apply` handlers) and
  its `ContextDecorator`/`ConfidenceHttpRequest`/`ConfidenceHttpResponse` types — ported onto the
  new `Resolve`/`ApplyFlags`; same request/response JSON.
- Metrics: expose the module's `metrics_snapshot` (Prometheus and OpenMetrics text) from the
  provider, replacing the `SnapshotConfig` placeholder.

## Shutdown

Idempotent, as today: stop, give `engine.close` a 10s budget (deadline-honoring per phase 6),
close the provider-owned remote store. The old executor cascade collapses into engine + store.

## Tests

- Port `provider_test.go`/`value_test.go`: evaluation of every type; typed defaults (merge
  semantics, all-or-nothing type checks, nulls→defaults); `Resolve`+`ApplyFlags` round-trip via
  the engine with `TestBackend`; `_confidence_skip_apply` per-evaluation and stripped everywhere;
  exposure modes end-to-end (logs seen/dropped); not-ready → ready events; fatal vs recoverable
  startup; config defaults.
- Keep and adapt: `ResolveTest`, `ApplyFlagsErrorHandlingTest`, `FlagResolverServiceTest`,
  `RequiredEncryptionTest`, shutdown-order test (now: close waits bounded for in-flight
  evaluations).
- E2E (`*E2ETest`/failsafe, secrets required, run only in `test-e2e`): port go's
  `TestE2E_NonSticky`, `TestE2E_MaterializedSegmentWithoutStore`, `TestE2E_StickyRemoteStore`,
  `TestE2E_MaterializedSegmentRemoteStore` against the production module CDN + ingestion.

## Done when

The provider's public surface matches this doc, unit tests cover everything above, and the old
resolver stack's classes are deleted from `src` in the same PR.


## Status: done ✅

`OpenFeatureLocalResolveProvider` rebuilt on the `Engine` and the old stack deleted (22 main
classes, 27 old tests, `resolver_state_current.pb`). Suite: 89 tests green in the harness
(40 internal from phases 3–6 + 49 provider-layer).

## Deltas from the Go reference

- **Typed evaluations throw → error codes.** The old Java provider threw OpenFeature exceptions
  (`FlagNotFoundError`, `TypeMismatchError`, `GeneralError`); the new one follows Go and returns
  the caller's default with `ErrorCode` + `reason ERROR` set on the evaluation (test assertions
  adapted accordingly). `initialize()` does throw `GeneralError` — on terminal startup (state
  FATAL) and on initialization timeout, while the engine keeps retrying and `onReady` flips the
  state to READY.
- **Int/Double defaults.** The OpenFeature Java `Value` does not separate int from double
  numbers; the mapper distinguishes them via the boxed object (`asObject() instanceof Integer`).
  The module rejects cross-scalar matches (an int flag with a double default is `TYPE_MISMATCH`,
  and vice versa), so the Go `count`-as-float case is not asserted.
- **Apply without send_time is accepted.** The state-to-wasm module carries an apply through
  without `send_time`; the old module's swallow-and-log path no longer exists. `applyFlags` still
  swallows and logs rejects (e.g. rotated secret → `UNAUTHENTICATED`).
- **`MaterializationStore` public interface unchanged** (CompletionStage-based sealed records);
  `StoreAdapter` joins reads (the engine is synchronous) and fire-and-forgets writes,
  unwrapping `MaterializationNotSupportedException` through the engine's cause chain to Go's
  GENERAL-code mapping. `RemoteMaterializationStore` is hand-encoded protobuf over HTTP exactly
  as Go's, with the same env-var timeouts.
- **`HttpURLConnection` factory → `java.net.http.HttpClient` factory.** `HttpClientFactory`
  supplies the one client the provider shares between CDN, ingestion and the remote store.
- **Surefire loads byte-buddy-agent via `-javaagent`** (dependency-plugin stages the jar) —
  Mockito's inline mock maker cannot self-attach on modern JDKs, which broke the ported
  `FlagResolverServiceTest`.
- **OpenFeature-API singleton test dropped** (go's `TestOpenFeature`): the Java
  `OpenFeatureAPI` singleton contaminates the test JVM; the wiring is covered by the
  metadata/initialize/shutdown tests instead.
- **`Sdk` identity**: `SDK_ID_JAVA_LOCAL_PROVIDER` (21) with `Version.VERSION`, as the old
  stack. Resolve tokens documented as neither encrypted nor authenticated.


## Self-review fixes (post-phase review)

- Evaluation `reason` now maps to the OpenFeature reasons (Go's `reason()`: `TARGETING_MATCH`,
  `DEFAULT`, `DISABLED`, `ERROR`, `UNKNOWN`) instead of the raw proto enum names — the deltas
  entry about "reason strings" above is superseded. Tests assert the mapped names.
- Default instances is `Runtime.availableProcessors()`, as the Go provider's GOMAXPROCS default
  (previously the engine's 1).
- Absent struct fields zero-fill as Go's `decodeTo` (the caller's default no longer leaks into
  fields the flag doesn't have).
- Remote-store variant results with no variant string decode as "no assignment" (dropped), as
  Go — instead of failing the whole read.
- A non-finite double default is a `TYPE_MISMATCH` evaluation, not a raw
  `IllegalArgumentException`; context instants serialize with full nanosecond precision, as Go's
  RFC3339Nano.
- The orphaned failsafe (shaded-jar IT) wiring is removed from the pom: no `*IT` classes remain
  and no target reaches `verify`; `make build` asserts the shaded jar's relocations instead.
