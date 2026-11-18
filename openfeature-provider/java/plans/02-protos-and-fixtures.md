# 02 — Protos and shared fixtures

Ports go commit `93c7365e` (generate the state-to-wasm protos and add test modules). One commit of
the rewrite PR.

## Goal

Generate the Java protobuf classes for the module API from the shared copies in
`openfeature-provider/proto`, embed the dedupe filter module, and pin the shared fixtures. The
module as a whole doesn't compile yet — the old stack stays until the provider commit (phase 7)
removes it, as Go's merge did; per-commit buildability isn't required.

## Changes

### pom.xml — proto generation

`protoSourceRoot` stays `${project.basedir}/../proto`. The include set is now only the
state-to-wasm groups (the old rust-guest groups no longer exist on `next`):

```xml
confidence/flags/resolver/v1/**/*.proto
confidence/flags/types/v1/**/*.proto
confidence/flags/admin/v1/**/*.proto
confidence/flags/wasm/v1/**/*.proto
```

- `protobuf-java`/`protoc` at **4.33.2**, paired with
  `com.google.api.grpc:proto-google-common-protos:2.65.0` — the pairing state-to-wasm builds with
  (2.65.0 needs a runtime with `com.google.protobuf.GeneratedFile`; 3.25.5 doesn't have it).
  Spike-verified end to end.
- The gRPC generation is gone: only `protobuf:compile`, no `compile-custom`, no grpc-java output
  dir. The protos keep their declared `java_package`s (`com.spotify.confidence.flags.*.v1`);
  shading relocates them for consumers.
- Also removed with the old stack's needs: `io.grpc:*`, guava, `javax.annotation-api`, the Chicory
  AOT compiler plugin and its wasm resource, the Chicory runtime/wasm deps (Endive arrives in phase
  3), and the grpc/guava entries from the shade config (protobuf, gson, common-protos stay).
- `mvn protobuf:compile` regenerates; the Makefile exposes it as `make proto`.

### Fixtures and resources

- Tests read `openfeature-provider/testdata` by relative path (Docker keeps the repository layout
  under `/src/openfeature-provider/...`, as the Go stages do) — fixtures are not copied into
  `src/test/resources`.
- The dedupe filter module is embedded at
  `src/main/resources/wasm/dedupe-filter-threaded.wasm`, copied from the Go host's asset (same
  state-to-wasm build as `testdata/`, compiler revision recorded in the testdata README). The
  host loads it in phase 3.

## Tests landing in this commit

`TestdataTest` (runs green once the module compiles — verified in a standalone harness while the
old stack still blocks compilation):

- the module fixture exists by relative path, is well-sized and carries the wasm magic;
- the state fixture carries the documented flags (`flags/plain`, `flags/sticky`,
  `materializedSegments/sticky`) — the state's message is state-to-wasm's own, not part of the
  provider protos, so the check reads the documented shapes rather than parsing a proto;
- `ClientModule` (the CDN artifact: module bytes + log token) round-trips;
- one type per generated package pins the pom's include set.

## Done when

- `mvn protobuf:compile` emits only the four state-to-wasm packages (`admin`, `resolver`, `types`,
  `wasm`).
- No gRPC generation, Chicory plugin, or grpc/guava dependency remains in the pom.
- `TestdataTest` is green (harness-verified now; in-module once phase 7 removes the old stack).
