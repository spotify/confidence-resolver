# Unit-Local Resolver (RFC / proposal)

**Status:** draft for discussion — prototype validated end-to-end on iOS simulator · **Owner:** Fabrizio Demaria · **Date:** May 2026

A hybrid between the online Java resolver (`epx-flags-resolver`) and the
fully-local WASM resolver (`confidence-resolver`), for clients that operate
on behalf of a **single fixed entity** but resolve flags against many
different evaluation contexts over time.

## TL;DR

For a fixed `user_id`, the WASM resolver can be served a **per-user state
slice** that omits the population bitsets (which dominate state size). A
prototype slicer rewrites a full `ResolverState` into a slice that the
**unmodified resolver** accepts — on the bundled fixture, **~2% of original
size** (246 KB → ~5 KB) with bit-identical resolve results. The full
loop — Rust slice server, Swift package wrapping the WASM resolver via
**WasmKit**, and a modified
[`spotify/confidence-sdk-swift`](https://github.com/spotify/confidence-sdk-swift)
running on the iOS simulator — has also been demonstrated end-to-end (see
[Branches and code references](#branches-and-code-references)).

## Problem

The WASM resolver ships the full account state to every client. State
size is dominated by **one 1M-bit population bitset per segment**.

For clients that operate on behalf of a **single fixed entity** — most
notably the **Confidence mobile SDKs**
([`spotify/confidence-sdk-swift`](https://github.com/spotify/confidence-sdk-swift)),
where `targetingKey` is set once per session but the rest of the
evaluation context churns as the user navigates — shipping the full
bitsets is wasteful, while staying online is high-latency, breaks
offline, and surfaces `STALE`.

## Idea: per-user state slicing

Unit identity enters resolution in exactly **two** places:

1. **Population check** — `bitset[hash("MegaSalt-{accountId}|{unit}") % 1_000_000]`
2. **Variant bucketing** — `hash("{segmentId}|{unit}") % bucket_count` → find
   the assignment whose `bucket_ranges` cover that bucket

For a fixed `unit`, both collapse to a single deterministic answer per
segment / rule, knowable at slice time.

### Slicing recipe

- **Population bitsets**: lookup the unit's bucket. If set → rewrite to
  `full_bitset: true` (resolver fast path bypasses the check). If clear →
  gzipped 1M-bit all-zero bitset (~140 bytes).
- **Variant assignments**: lookup the unit's variant bucket, find the
  covering assignment, rewrite its range to `[0, bucket_count)`. Whatever
  bucket the resolver hashes to lands on the right variant.

Everything else is untouched; the resolver never knows it's running on a
slice.

## Prototype and validation

Slicer: [`confidence-resolver/src/slicer.rs`](confidence-resolver/src/slicer.rs)
— public API `slice_for_unit(state, account_id, unit) -> ResolverState`.
No protobuf changes, no WASM entry-point changes, no resolver-core changes.

Integration test:
[`confidence-resolver/tests/per_user_slice.rs`](confidence-resolver/tests/per_user_slice.rs).
4 pinned units × 4 evaluation contexts, resolved against both the full
state and the slice; non-deterministic fields stripped before comparison.

```text
per-user slicer parity test
  fixture: resolver_state.pb (245622 bytes)
  unit=tutorial_visitor   sliced=   5093 bytes ( 2.07% of original)
  unit=user_42            sliced=   4935 bytes ( 2.01% of original)
  unit=alice              sliced=   5093 bytes ( 2.07% of original)
  unit=bob_12345          sliced=   5093 bytes ( 2.07% of original)
test slice_preserves_resolve_results ... ok
```

- **Identical resolve results** across all 16 (unit × context) pairs.
- **~50× size reduction** on this fixture; savings scale with segment count.

The slicer's math is selector-agnostic. The prototype also ships a
defensive consistency check that aborts on multi-selector accounts; with
the SDK API in §B item 5, that check is no longer load-bearing — see open
question #5.

### End-to-end validation on iOS

Beyond the slicer parity test, the full loop is now wired up locally
across two `unit-local` branches (see
[Branches and code references](#branches-and-code-references)):

- **Slice server** (`unit-local-server/`, Rust + axum) — exposes
  `/v1/account-config` and `/v1/resolver-state/{stateHash}/{unit}` from a
  bundled fixture by invoking `slicer::slice_for_unit` per request.
- **Swift package** (`openfeature-provider/swift/`) — wraps
  `confidence_resolver.wasm` via **WasmKit** (pure-Swift WASM runtime,
  iOS 16+), ports the `wasm-msg` allocator + protobuf envelopes to
  Swift, exposes a typed `LocalResolver` plus a `SliceClient`.
- **SDK glue** — `LocalConfidenceResolveClient` in
  [`spotify/confidence-sdk-swift`](https://github.com/spotify/confidence-sdk-swift/tree/unit-local):
  a Swift `actor` implementing the SDK's existing
  `ConfidenceResolveClient` protocol against the local resolver. Refetches
  the slice on randomization-unit changes; converts
  `ConfidenceStruct` ↔ `Google_Protobuf_Struct` for both context and
  resolved values.
- **Demo app** — `ConfidenceDemoApp` rewritten to a single SwiftUI screen
  that bootstraps the local resolver against `http://127.0.0.1:8787`.
  On the iPhone 17 simulator, `fallthrough-test-1.enabled` resolves to
  `true` with reason `match`; slice on the wire for `user_42` is
  **4,935 / 245,717 B ≈ 2%**, matching the parity test.

A few concrete findings beyond what the slicer-only prototype could show:

- **SDK surface change is genuinely tiny.** One new file (~200 LoC) plus
  one public builder method (`Confidence.Builder.withLocalResolver(_:)`).
  The SDK's flag cache, evaluation, apply scheduling, and OpenFeature
  paths are untouched. The existing `ConfidenceResolveClient` protocol —
  a single async method — is the entire integration point, which is
  strong evidence the swap is additive rather than invasive.
- **Platform cost is real.** WasmKit lifts the SDK's minimum to **iOS 16
  / macOS 14**. Apple has no first-party iOS WASM runtime today; WasmKit
  is the only credible pure-Swift, SPM-friendly option found. Wasmer /
  Wasmtime have no iOS targets.
- **Apply path is the unfinished bit.** The SDK's default `FlagApplier`
  keeps POSTing to the live backend in local mode; resolves succeed
  regardless because apply is fire-and-forget, but a productionised
  unit-local mode needs the apply path pointed at the slice service or a
  local sink (see open question #6).
- **Generated protos are bulky on iOS.** ~3.9 kLOC of SwiftProtobuf
  bindings, plus a naming collision between two `types.proto` files in
  different proto packages worked around with
  `FileNaming=PathToUnderscores`. Java/Go don't have this because their
  codegen is package-aware. A real build should generate at build time
  rather than vendor.
- **Swift 6 concurrency turned out to be a non-issue.** An NSLock
  approach got compiler-flagged; converting to an `actor` was a one-line
  change and the existing `async` protocol requirement satisfied
  without any nonisolated wrappers. The local resolver's concurrency
  semantics are arguably cleaner than the remote one's.

## What it would take to ship this

Two new pieces, plus the existing slicer: a server endpoint that serves
per-user slices, and a WASM-enabled iOS SDK that consumes them.

### A. Server: per-user state endpoint

New endpoint accepting `(client_secret, unit)` and returning a serialized
sliced `ResolverState`. Natural home: `epx-flags-resolver`, which already
caches the full account state in memory via
[`InternalFlagsAdminFetcher`](../epx-flags-resolver/epx-flags-resolver-service/src/main/java/com/spotify/confidence/flags/resolver/repository/InternalFlagsAdminFetcher.java).
The slicer can be ported to Java or invoked as the Rust WASM.

Reuse existing `client_secret` auth and gRPC stack. Cache on
`(account, stateFileHash, unit)`; `stateFileHash` invalidates naturally on
admin state changes.

### B. Client: WASM-enabled iOS SDK

The current Swift SDK is an online provider; unit-local mode requires
the following. Items 1–4 have been prototyped on the
[`unit-local` branch](https://github.com/spotify/confidence-sdk-swift/tree/unit-local)
and exercised end-to-end on the simulator (see
[End-to-end validation on iOS](#end-to-end-validation-on-ios) above);
items 5–6 remain design-stage.

1. **WASM runtime** — embed e.g. **WasmKit** (pure Swift, Apple-backed).
2. **WASM binary distribution** — bundle in SDK release; rely on
   `wasm-msg`'s host↔guest version handshake. (`confidence_resolver.wasm`
   is ~470 KB committed.)
3. **Host bridge** — implement the two host imports
   (`wasm_msg_host_log_message` → `os_log`,
   `wasm_msg_host_current_time` → `Date()`).
4. **Message layer** — port the `wasm-msg` allocator + protobuf round-trip
   to Swift (`SwiftProtobuf`). Reference:
   [`WasmLocalResolver.java`](openfeature-provider/java/src/main/java/com/spotify/confidence/sdk/WasmLocalResolver.java).
5. **New SDK API: explicit randomization unit.** Unit-local mode
   introduces a cost asymmetry today's API hides — identity change costs
   network, anything else is local-only. Conflating both inside
   `setEvaluationContext` would be a footgun. Proposal:

   ```swift
   confidence.setRandomizationUnit("user_42")   // triggers slice fetch
   confidence.setEvaluationContext(...)         // local-only, no network
   ```

   `setRandomizationUnit` takes a **value only**. The field name(s) the
   value should land in at resolve time travel to the SDK as slice
   response metadata (`randomization_unit_fields: ["visitor_id"]`); the
   SDK auto-injects the value into every listed field before each
   resolve. Multi-selector accounts are handled transparently; app code
   stays unaware of which field name(s) the portal happens to use.

   ```mermaid
   sequenceDiagram
       autonumber
       participant App as iOS app
       participant SDK as Swift SDK
       participant Slice as Slice API
       participant Wasm as WASM resolver

       App->>SDK: setRandomizationUnit("user_42")
       SDK->>Slice: GET /v1/resolver-state/{account}/{stateHash}/user_42
       Slice-->>SDK: { resolver_state,<br/>randomization_unit_fields: ["visitor_id"] }
       SDK->>Wasm: set_resolver_state(bytes)

       Note over App,Wasm: User navigates; context changes

       App->>SDK: setEvaluationContext({ country: "SE", ... })
       SDK->>SDK: Inject "user_42" into selector fields
       App->>SDK: getStringValue("...")
       SDK->>Wasm: resolve_flags(ctx)
       Wasm-->>SDK: ResolvedFlag(...)

       Note over App,Wasm: Network only at step 2.
   ```

   Keep the online path as a configurable fallback.

6. **Cache invalidation** — slice bytes carry `state_file_hash`; SDK
   refetches only when it changes.

### C. Performance and caching

Slices are computed **on the fly** — per-user fan-out makes upfront
pre-computation impractical.

Cost per slice with account state warm in memory: ~500 segments ×
(murmur3 + modulo + bit lookup), plus an assignment rewrite per rule and a
sub-ms protobuf encode → **sub-millisecond total**.

Recommended shape: **API endpoint + CDN edge cache**, with
`state_file_hash` baked into the URL
(`GET /v1/resolver-state/{accountHash}/{stateFileHash}/{unitHash}`). First
request: CDN miss → origin computes → edge caches. State change: new
hash → new URL → old slices age out without explicit purge.

### Effort estimate (rough)

| Piece | Estimate |
|---|---|
| Server slice endpoint + caching | 1–2 weeks |
| Swift WASM host + `wasm-msg` port + local-resolve flow | 2–3 weeks |
| Swift SDK unit-local mode behind a flag, online fallback | 1–2 weeks |
| E2E QA on iOS (offline, context churn, refresh) | 1 week |
| **Total** | **~5–8 weeks** for a behind-flag beta |

Android would follow the same pattern with Chicory or similar.

## Open questions for the team

1. **Where should the slicer run?** New endpoint on `epx-flags-resolver`
   (already has account state in memory)? A new dedicated service? Inside
   the client (fetch full state once, re-slice locally per user)?
2. **Privacy / trust boundary.** For the target Swift SDK migration this
   is a **net win**: the server sees `user_id` once at slice time instead
   of on every resolve, as it does today. Compared to the fully-local
   providers (JS / Java / Go / Python) it's a regression, but those
   aren't viable on mobile due to state size. Pseudonymisation (hash the
   unit before sending) remains available if customers want stronger
   guarantees — the resolver math is identical.
3. **Per-user proto?** The POC reuses `PackedBitset` so the resolver
   loads slices unchanged. A purpose-built
   `PerUserResolverState { map<segment_name, bool> }` would simplify the
   payload and unlock further size reductions, at the cost of a new WASM
   entry point.
4. **Slice staleness across admin changes.** The slicer bakes the
   user's variant into the rewritten `bucket_ranges`, so admin changes
   to a rule's allocation make existing slices yield the **old** answer
   until the next refresh — same staleness shape as today's full-state
   WASM cache (also keyed by `stateFileHash`). Acceptable for the iOS
   use case: clients refresh on session start, and within-session
   stability is desirable (no flicker from server-side changes that the
   app didn't trigger). Worth noting in docs; not a blocker.
5. **Multi-selector accounts and the slicer's check.** With the SDK API
   in §B item 5, multi-selector accounts work transparently. The slicer's
   consistency check is therefore defensive only — keep it as
   defence-in-depth, or drop it and let the SDK contract be authoritative?
6. **Sticky assignments / materializations.** `read_materialization`
   rules pull per-user records from Bigtable at resolve time; we'd need
   to **prefetch them into the slice** so the local resolver has them.
   Writes are already handled — the iOS SDK has a dedicated `apply` code
   path that reports back to the backend, and the backend writes the
   materialization server-side as it does today.

   *Surfaced during E2E*: the SDK's default `FlagApplier` still POSTs to
   the live backend even in local mode (resolves succeed regardless
   because apply is fire-and-forget, but a steady stream of network
   errors goes silently into the void). Productionising unit-local mode
   needs the apply path pointed at the slice service — or a local sink
   plus periodic flush — rather than the existing online endpoint.

## Branches and code references

All prototype work lives on `unit-local` branches in two repos. No PRs
opened — these are intended as readable code diffs for the team.

[`spotify/confidence-resolver` — `unit-local`](https://github.com/spotify/confidence-resolver/tree/unit-local)
&nbsp;([compare with `main`](https://github.com/spotify/confidence-resolver/compare/main...unit-local))

| Path | Purpose |
|---|---|
| `confidence-resolver/src/slicer.rs` | `slice_for_unit(state, account_id, unit)` — the core rewriter |
| `confidence-resolver/tests/per_user_slice.rs` | Parity test, 4 units × 4 contexts |
| `unit-local-server/` | Local Rust HTTP slice server (axum) on `127.0.0.1:8787` |
| `openfeature-provider/swift/Sources/ConfidenceLocalResolver/` | WasmKit-backed `LocalResolver`, host-side `wasm-msg`, `SliceClient` |
| `openfeature-provider/swift/Sources/LocalResolverCli/` | Standalone CLI exercising the same flow as the iOS demo |

[`spotify/confidence-sdk-swift` — `unit-local`](https://github.com/spotify/confidence-sdk-swift/tree/unit-local)
&nbsp;([compare with `main`](https://github.com/spotify/confidence-sdk-swift/compare/main...unit-local))

| Path | Purpose |
|---|---|
| `Sources/Confidence/LocalConfidenceResolveClient.swift` | `actor` implementing `ConfidenceResolveClient` against the local resolver |
| `Sources/Confidence/Confidence.swift` (`withLocalResolver(_:)`) | Public builder hook to inject the local resolver |
| `ConfidenceDemoApp/ConfidenceDemoApp/ConfidenceDemoApp.swift` | Rewritten demo: SwiftUI screen that resolves a flag against the local server |
| `Package.swift` | Adds the local-path SPM dep on the resolver Swift package; bumps platforms to iOS 16 / macOS 14 |

## References

- Resolver core (hashing, bucketing): [`confidence-resolver/src/lib.rs`](confidence-resolver/src/lib.rs)
- Java equivalent of the slicer math: [`Randomizer.java`](../epx-flags-resolver/epx-flags-resolver-lib/src/main/java/com/spotify/confidence/flags/resolver/Randomizer.java)
- Java WASM provider (reference for the Swift port): [`WasmLocalResolver.java`](openfeature-provider/java/src/main/java/com/spotify/confidence/sdk/WasmLocalResolver.java)
- Swift / iOS SDK: [`spotify/confidence-sdk-swift`](https://github.com/spotify/confidence-sdk-swift)
- WasmKit (Swift WASM runtime): [`swiftwasm/WasmKit`](https://github.com/swiftwasm/WasmKit)
