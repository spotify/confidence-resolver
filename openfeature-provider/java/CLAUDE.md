# Java OpenFeature Provider

## Overview

Maven coordinates: `com.spotify.confidence:openfeature-provider-local`

Java OpenFeature provider resolving flags locally in a state-to-wasm **flag module**: a
WebAssembly module compiled per client credential from that client's flags, fetched encrypted
from the Confidence CDN and run on **Endive** (the JVM-native wasm runtime, Chicory's successor;
compiled to JVM bytecode by default, interpreter fallback).

## Architecture

- **`com.spotify.confidence.sdk`** — public API: `OpenFeatureLocalResolveProvider` (typed
  evaluations through one path), `LocalProviderConfig`, `MaterializationStore` (sealed records,
  CompletionStage-based), `RemoteMaterializationStore` (hand-encoded protobuf over HTTP),
  `FlagResolverService` + `ContextDecorator` (JSON resolve/apply proxy for client SDKs), and
  package-private `OpenFeatureValueMapper`/`StoreAdapter`.
- **`internal.host`** — runs a module on Endive: validation, capped memory, a pool of
  instances, resolve/evaluate/apply/flush calls, the host imports (`write_logs`, `current_time`,
  materialization reads/writes), and the dedupe filters, linked to the module wasm-to-wasm.
- **`internal/cdn`** — fetches and decrypts the credential's module (AES-256-GCM), with ETags.
- **`internal/ingest`** — ships flag logs to `ingestion.confidence.dev/v1/flagLogs:write`, with
  the module's log token as Bearer.
- **`internal/engine`** — module generations: polls the CDN, swaps in a new module without
  dropping evaluations, flushes logs on a timer and on swap and shutdown.
- **`internal/testutil`** — a fake CDN and ingestion server for tests (`TestBackend`).

Protobuf classes generate from `../proto` into `target/generated-sources/protobuf/java` (never
edit; `make proto` regenerates). Test modules come from `../testdata/modules`. Event tracking
(`track()`) isn't supported yet; it will come through the module.

## Build & Test

```bash
make build      # mvn verify -DskipTests (shaded jar + dependency analysis)
make test       # unit tests (excludes *E2ETest)
make test-e2e   # needs CONFIDENCE_CLIENT_SECRET and CONFIDENCE_CLIENT_ENCRYPTION_KEY
make lint       # mvn fmt:check
make proto      # regenerate the protobuf classes from ../proto
```

## Gotchas

- **Proto location**: sources are at `../proto` (i.e. `openfeature-provider/proto/`), NOT at
  `../../confidence-resolver/protos/`.
- **Java 17**: the pom compiles with `maven.compiler.release=17` — no pattern switches over
  sealed records (write `instanceof` chains); enum switches and type patterns are fine.
- **Sequential surefire**: `forkCount=1`, `reuseForks=false` — one test JVM at a time.
- **Mockito**: the inline mock maker cannot self-attach on modern JDKs; the pom stages
  `byte-buddy-agent` into `target/` and surefire loads it via `-javaagent`.
- **Shaded JAR**: Endive, ASM, protobuf, gson and common-protos are relocated under
  `com.spotify.confidence.sdk.shaded` — unit tests run against plain classes; check shading
  changes via `make build` (the shade plugin runs in package).
- **Secrets in Docker**: mounted as Docker secrets, never written to a layer (see the root
  Dockerfile's `test_e2e` stages).
