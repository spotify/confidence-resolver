# Java OpenFeature Provider

## Overview

Maven coordinates: `com.spotify.confidence:openfeature-provider-local`

Java OpenFeature provider using the Confidence resolver compiled to WASM, with Chicory AOT compilation for near-native performance.

## Key Architecture

- **Chicory WASM AOT** — The WASM binary (`src/main/resources/wasm/confidence_resolver.wasm`) is AOT-compiled to Java bytecode at build time via `chicory-compiler-maven-plugin`. This generates `com.spotify.confidence.sdk.ConfidenceResolverModule`.
- **gRPC** — Used for flag log shipping and materialization store communication. Protobuf + gRPC stubs are generated from `../proto/`.
- **Shaded JAR** — gRPC, protobuf, and guava are relocated to `com.spotify.confidence.sdk.shaded.*` to avoid version conflicts with consumers.

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
make build      # build WASM (if needed) + mvn package -DskipTests
make test       # build + mvn test (excludes *E2ETest)
make test-e2e   # build + mvn verify (integration tests against shaded JAR)
```

## Gotchas

- **Proto location**: Sources are at `../proto/` (i.e., `openfeature-provider/proto/`), NOT at `../../confidence-resolver/protos/`.
- **Generated code**: Goes to 3 separate directories — `target/generated-sources/protobuf/java`, `target/generated-sources/protobuf/grpc-java`, and `target/generated-sources/chicory-compiler`.
- **Integration tests** (failsafe, `*IT.java`): Run against the **shaded JAR** to verify shading works correctly. If shading breaks, unit tests pass but integration tests fail.
- **Sequential tests**: Surefire runs with `forkCount=1` (sequential execution).
- **Publishing**: Uses `central-publishing-maven-plugin` (not nexus-staging). Secrets are mounted during Docker build only — never written to layers.
