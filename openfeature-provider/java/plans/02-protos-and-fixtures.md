# 02 — Protos and shared fixtures

Ports go commit `93c7365e` (generate the state-to-wasm protos and add test modules). One PR.

## Goal

Generate the Java protobuf classes for the module API from the shared copies in
`openfeature-provider/proto`, and wire the shared test fixtures so tests can run modules without a
compiler. The provider still doesn't build as a whole after this PR — only the generated code and
fixtures land.

## Changes

### pom.xml — proto generation

`protoSourceRoot` stays `${project.basedir}/../proto`. Replace the include set (the old rust-guest
groups no longer exist on `next`):

```xml
<!-- remove -->
confidence/events/v1/**/*.proto
confidence/events/wasm/v1/**/*.proto
confidence/wasm/*.proto

<!-- keep / add -->
confidence/flags/resolver/v1/**/*.proto
confidence/flags/types/v1/**/*.proto
confidence/flags/admin/v1/**/*.proto
confidence/flags/wasm/v1/**/*.proto
```

- Drop the `compile-custom` (gRPC) goal and the grpc-java output dir — nothing generates gRPC
  anymore; only `protobuf:compile` remains. Keep protoc at a version that builds the
  `google.api.field_behavior` / `google.type.decimal` imports (both live in `../proto`).
- The protos declare `java_package` per file (`com.spotify.confidence.flags.*.v1`) — keep the
  declared packages; shading relocates them for consumers, as today.
- Keep `build-helper` adding `generated-sources/protobuf/java` (drop the `grpc-java` source dir
  and, later in phase 8, the endive-compiler one, if build-time compilation remains useful).
- Add a `make proto` entry (phase 8 defines the Makefile contract fully) that re-runs the
  generation, mirroring `openfeature-provider/go/scripts/generate_s2w_proto.sh`.

### Fixtures

- Tests read `openfeature-provider/testdata` by relative path (Docker keeps the repository layout
  under `/src/openfeature-provider/...`, as the Go stages do) — do **not** copy fixtures into
  `src/test/resources`. The old `resolver_state_current.pb` fixture is deleted with the old
  resolver stack.
- The dedupe filter module (`dedupe-filter-threaded.wasm`, ~1.9 KB, state-to-wasm build — the Go
  host embeds it at `internal/host/assets/`) is embedded as a **resource** at
  `src/main/resources/wasm/dedupe-filter-threaded.wasm`. Copy it from the same state-to-wasm
  revision recorded in `openfeature-provider/testdata/README.md` and say so in a short comment in
  the host's `filter` class when phase 3 lands.

## Tests landing in this PR

- A small test that parses `ClientModule` (the artifact proto: `module` bytes + `log_token`) and
  `ResolverState` from `../testdata/states/basic.json` via `JsonFormat`, asserting the
  `flags/plain` and `flags/sticky` shapes it documents.
- A test that every generated package resolves (compiles).

## Done when

- `mvn generate-sources` emits only state-to-wasm classes; `make proto` regenerates them.
- No reference to `confidence/events`, `confidence/wasm` or gRPC generation remains in the pom.
- The fixture test runs green against `../testdata` from a plain `mvn test` in
  `openfeature-provider/java`.
