# Java OpenFeature Provider

## Overview

Maven coordinates: `com.spotify.confidence:openfeature-provider-local`

Java OpenFeature provider using the Confidence resolver compiled to WASM, with Chicory AOT compilation for near-native flag resolution and a separate event-engine WASM for OpenFeature tracking.

## Key Architecture

- **Chicory WASM AOT** — The WASM binary (`src/main/resources/wasm/confidence_resolver.wasm`) is AOT-compiled to Java bytecode at build time via `chicory-compiler-maven-plugin`. This generates `com.spotify.confidence.sdk.ConfidenceResolverModule`.
- **Resolver pool and recovery** — A configurable pool (default 2, capped at available processors) wraps recovering resolver instances.
- **Event tracking** — `confidence_event_engine.wasm` is loaded through Chicory at runtime; tracked events are flushed every 15 seconds and published over gRPC.
- **Transport** — Flag logs use destination-aware gRPC/HTTP delivery. gRPC is also used for event publishing and remote materializations. Protobuf + gRPC stubs are generated from `../proto/`.
- **Shaded JAR** — gRPC, protobuf, and guava are relocated to `com.spotify.confidence.sdk.shaded.*` to avoid version conflicts with consumers.

## Configuration

`LocalProviderConfig.builder()` supports custom channel and HTTP client factories, remote materializations, resolver pool size, an optional AES-256 state encryption key, experimental apply deduplication, and disabling exposure collection.

## Main Provider Class

```java
package com.spotify.confidence.sdk;

public class OpenFeatureLocalResolveProvider implements FeatureProvider {
    public OpenFeatureLocalResolveProvider(String clientSecret) { ... }
    public OpenFeatureLocalResolveProvider(LocalProviderConfig config, String clientSecret) { ... }
    public OpenFeatureLocalResolveProvider(String clientSecret, MaterializationStore materializationStore) { ... }
    public OpenFeatureLocalResolveProvider(LocalProviderConfig config, String clientSecret, MaterializationStore materializationStore) { ... }
}
```

## Build & Test

```bash
make build      # build both WASM resources (if needed) + mvn package -DskipTests
make test       # build + mvn test (excludes *E2ETest)
make test-e2e   # build + mvn verify (integration tests against shaded JAR)
```

## Gotchas

- **Proto location**: Sources are at `../proto/` (i.e., `openfeature-provider/proto/`), NOT at `../../confidence-resolver/protos/`.
- **Generated code**: Goes to 3 separate directories — `target/generated-sources/protobuf/java`, `target/generated-sources/protobuf/grpc-java`, and `target/generated-sources/chicory-compiler`.
- **Integration tests** (failsafe, `*IT.java`): Run against the **shaded JAR** to verify shading works correctly. If shading breaks, unit tests pass but integration tests fail.
- **Sequential tests**: Surefire runs with `forkCount=1` (sequential execution).
- **Publishing**: Uses `central-publishing-maven-plugin` (not nexus-staging). Secrets are mounted during Docker build only — never written to layers.
