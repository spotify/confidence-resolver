# confidence-event-engine

Batches OpenFeature `track()` events inside a WebAssembly module so every
server-side provider (JS, Java, Go, Python) shares one implementation of the
batching and payload-mapping rules.

It mirrors the resolver's `AssignLogger`: a lock-free `SegQueue` for `track()`,
and a `Mutex`-guarded pending buffer for size-bounded flushes.

```
Provider.track(name, context, details)
        |
        v
  confidence_event_engine.wasm
  ├── wasm_msg_guest_track_event          queue one event
  └── wasm_msg_guest_bounded_flush_events drain <= 2 MB, return a batch
        |
        v
Provider publishes the batch (gRPC, or HTTP for JS)
```

## Payload mapping

`build_payload` (in `wasm/event-guest`) merges the OpenFeature inputs into the
Confidence event payload in this order:

1. `data` — the caller's custom fields
2. `value` — the OpenFeature numeric value
3. `context` — the evaluation context

Order matters: **`value` and `context` are reserved keys and overwrite
same-named keys from `data`.** OpenFeature custom data may contain arbitrary
keys, so a caller passing `data: {"value": ...}` will see it replaced. This is
intentional and covered by unit tests in `wasm/event-guest/src/lib.rs`.

`value` is `optional double` in the proto specifically so an explicit `0` is
distinguishable from "not set" — a plain `double` would conflate them.

## Delivery guarantees

**Events are delivered at-most-once, and a failed publish drops the batch.**

Once `bounded_flush_events` returns, the events are gone from the WASM buffer.
If the subsequent publish fails, the provider logs and moves on; there is no
re-queue, dead-letter queue, or persistence.

This matches the flag-log path, which behaves the same way. In both cases the
mitigations are transport-level rather than application-level:

- **Retry.** gRPC providers attach a `retryPolicy` (3 attempts, 1s→10s backoff,
  ×2, on `UNAVAILABLE`); JS retries at the fetch layer. So a transient blip is
  usually absorbed before it reaches the drop path.
- **Observability.** Failures are counted and a warning is logged every 10
  attempts rather than once per failure, so a sustained outage is visible
  without flooding logs.

Consequences worth knowing before relying on this for billing-grade data:

- A hard failure (bad credentials, wrong endpoint) silently drops every event
  for as long as it persists — only the periodic warning surfaces it.
- Events buffered when the process dies uncleanly are lost. Shutdown drains up
  to 100 batches, but a `SIGKILL` skips that entirely.

## Known provider differences

**Go cannot distinguish `value: 0` from an unset value.** Go's
`openfeature.TrackingEventDetails` stores `value` as a plain `float64` with no
"is set" flag, and `NewTrackingEventDetails(v)` is the only constructor. Java
(`Optional<Number>`) and JS (`number | undefined`) can tell them apart and
forward an explicit `0` correctly.

The Go provider therefore treats `0` as unset and omits it. The alternative —
always sending — would attach a spurious `value: 0` to every event where the
caller set none, which is the far more common case. If you need to record a
zero-valued event from Go, put it in the custom data instead of `value`.

## Build and test

```bash
cargo test -p confidence-event-engine   # batching, bounds, concurrency
cargo test -p event-guest               # payload mapping and collision rules
make -C wasm/event-guest build          # the wasm32 artifact
```

The committed binary embedded by the Go provider must be produced in Docker
(`make sync-wasm-event-go`); rustc bakes absolute source paths into panic
strings, so a host build is never byte-identical. CI enforces this via the
`openfeature-provider-go.validate-event-wasm` stage.
