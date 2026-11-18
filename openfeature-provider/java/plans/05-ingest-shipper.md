# 05 — Flag-log shipper

Ports go commit `7b2a45ec`. One commit of the rewrite PR. Reference: `internal/ingest/shipper.go` on `next` (merged as #700).

## Goal

`com.spotify.confidence.sdk.internal.ingest.LogShipper` forwards serialized
`WriteFlagLogsRequest`s **verbatim** to ingestion, asynchronously, from a bounded queue that drops
(and counts) instead of blocking. The wasm `write_logs` callback must never block — the shipper is
the reason it can stay non-blocking.

## Behavior

- `POST {base}/v1/flagLogs:write`, base default `https://ingestion.confidence.dev`; body = the
  serialized bytes exactly as the module produced them (possibly a concatenation of fragments —
  the wire format parses concatenated encodings; do not re-encode); headers:
  `Content-Type: application/x-protobuf`, `Authorization: Bearer <log token of the generation that
  produced the batch>`.
- Queue: bounded (Go: 64 batches); `enqueue(bytes, token)` never blocks — on a full queue it drops
  and increments a drop counter; drops are surfaced as a metric/log line.
- Delivery: retries network errors, 429 and 5xx (Go: 3 attempts, 5s per-attempt timeout, 200 ms
  then 400 ms backoff); other 4xx are dropped (logged) — no retry.
- `close(deadline)`: stop accepting, drain what's queued bounded by the deadline, then shut the
  executor down. A closed shipper drops immediately.
- One background delivery thread is enough (Go uses a goroutine per shipper); deliveries are
  sequential per shipper.

## Java shape

- A single-thread `ExecutorService` (daemon), an `ArrayBlockingQueue<byte[]>`-style slot holding
  payload + token (Go: `batch{payload, token}`), counters (dropped, delivered, failed) exposed for
  tests and later metrics.
- Injected `HttpClient`/transport for tests, same seam as the CDN fetcher.

## Tests (port `shipper_test.go`)

- Payload + token reach the wire unchanged; content-type header set.
- Bearer token comes from the batch, not the constructor — two generations with different tokens
  (engine swaps) ship correctly.
- Retry: 500 then 200 succeeds; 429 then 200 succeeds; three failures drop the batch.
- 400 drops without retry; network exception retries.
- Full queue: enqueue drops and counts, never blocks the caller (assert with a deadline).
- Close: drains queued batches; after close, enqueue drops; close honors its deadline when a
  delivery hangs (fake server that stalls).

## Test helper landing here (port `internal/testutil/backend.go`)

`TestBackend` (test sources): a local HTTP server pair — a fake module CDN (status control,
ETag/`If-None-Match`, encrypted module publishing via a test key) and a fake ingestion (captures
bearer tokens and bodies). Shared by the engine and provider tests. `Publish`, `setStatus`,
`logs`, `hexKey`, `module(bytes)` helpers. This is the Java twin of Go's `testutil.Backend`.

## Done when

Shipper behavior matches Go's on every case above, and `TestBackend` can serve a published
encrypted `basic.wasm` round-trip (used by phases 6–7).
