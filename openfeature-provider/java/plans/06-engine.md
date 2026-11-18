# 06 — Engine: module generations and swap

Ports go commit `fe0ddf42` (engine) with the close fix `e821478f`. One commit of the rewrite PR. Reference:
`internal/engine/engine.go` on `next` (merged as #700).

## Goal

`com.spotify.confidence.sdk.internal.engine.Engine` owns the long-lived parts: the CDN fetcher,
the log shipper, the dedupe filters, and the current module generation. It swaps generations as
new modules are published, without dropping evaluations.

## Shape

```java
final class Engine {
  // config: pollInterval (10s), flushInterval (30s), retryInterval (1s),
  //         instances, maxMemoryBytes, exposure, clientSecret, key,
  //         cdnBase, ingestBase, httpClient, logger, onReady
  Engine.start();            // startup loop; returns fatal errors, or times out recoverably
  Engine.evaluate(...)       // -> List<ResolutionDetails>
  Engine.resolve(...)        // -> ResolveFlagsResponse (deferred apply path)
  Engine.applyFlags(...)
  Engine.metrics(openMetrics)
  Engine.close(deadline)
}
```

- `ExposureMode { DEDUPLICATED, ALL, NONE }` (public API mirrors it in phase 7): `ALL` → apply
  filter `NeverSeen`, `NONE` → `AlwaysSeen`, default → persistent dedupe filter. The schema
  filter is independent of the mode and always a dedupe filter. Filter memories: apply 8 pages,
  schema 2 pages — "as in the Java host" (state-to-wasm's own Java host; keep the sizes).
- `Generation { ReentrantReadWriteLock lock; Resolver resolver; String etag; }`; `current` is an
  `AtomicReference<Generation>`. An evaluation `readLock().tryLock()`; on failure it reloads the
  reference and retries — a generation being retired never serves new evaluations.
- **Swap:** on a changed module — build the new generation (compile, validate, instantiate,
  warm), publish it via the atomic reference, then retire the old: unpublish, `writeLock()` and
  never release. In-flight evaluations drain on their read locks. On any failure building the new
  generation: keep the current one, don't accept the ETag (the fetcher refetches in full).
- **Startup:** loop fetch/build every `retryInterval` within `initializationTimeout`; on expiry
  return a *recoverable timeout* (background polling continues; `onReady` fires when a module
  finally installs — the provider maps this to `PROVIDER_READY`). Terminal fetch outcomes and an
  invalid first module are *fatal*: stop and surface the cause.
- **Poll loop:** every `pollInterval`, conditional fetch. `UNCHANGED` → nothing; new module →
  swap; retryable error → keep serving, log; terminal error after a module is installed → keep
  serving, log loudly (only the *first* fetch's terminal errors are fatal).
- **Flush loop:** every `flushInterval` call the current generation's `FlushLogs` (payloads reach
  the shipper via the host's `write_logs` callback). Also flush on every swap (the old generation
  flushes during retirement, shipping with **its own** log token) and on close.
- **Close (deadline-honoring):** stop loops, retire the current generation **bounded by the
  deadline** — an evaluation parked in a slow materialization read must not outlive the caller's
  deadline: if the deadline hits, return the error and let the rest (retired generation's flush,
  shipper close, filter close) finish in the background. Nothing still in use is freed. New
  evaluations fail with closed. Idempotent.
- The shipper and the filters outlive every generation; the engine closes them after the last
  generation retires.

## Java notes

- Loops on one daemon `ScheduledExecutorService`; the poll/flush tasks must not overlap
  themselves.
- No `RecoveringResolver`: there is nothing to replay — a broken module call surfaces as an
  evaluation error; the generation is replaced only by a successful swap.

## Tests (port `engine_test.go`)

- Startup: first module installs → `onReady`; terminal first fetch → fatal; slow CDN →
  recoverable timeout, then ready later; invalid first module → fatal.
- Swap under load: evaluations in flight across a swap complete on the old generation; new
  evaluations immediately use the new one; no evaluation is dropped or duplicated.
- Broken new module: current generation keeps serving; ETag not accepted → next fetch is full.
- Filters: dedupe history survives a swap (same hash not re-logged across generations); exposure
  modes select the right filter pair.
- Flush: timer flush, flush-on-swap ships the old generation's logs with the old token, final
  flush on close.
- Close deadline: an evaluation stuck in a materialization read + `close(shortDeadline)` returns
  the error while cleanup completes in the background; `close` idempotent; everything frees (no
  thread/executor leaks).

## Done when

The engine, fed by `TestBackend` (phase 5) and `basic.wasm`, drives all of the above deterministically
in tests, including swap-under-load with concurrent evaluators.
