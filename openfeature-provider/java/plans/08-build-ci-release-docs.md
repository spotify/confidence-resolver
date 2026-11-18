# 08 — Build, CI, release and docs: landing the provider

Ports go commits `42610a23` (Docker stages, Makefile contract, release candidates) and `b8df8180`
(README/CLAUDE.md for the flag-module provider). One PR; the last one.

## Makefile contract (root `CLAUDE.md`, commit `09392989`)

`openfeature-provider/java/Makefile` offers, calling the Maven toolchain:

- `build` — `mvn -q package -DskipTests` (`.DEFAULT_GOAL`)
- `test` — unit tests only; surefire **skips** e2e (`*E2ETest` excluded)
- `test-e2e` — fails fast if `CONFIDENCE_CLIENT_SECRET` or `CONFIDENCE_CLIENT_ENCRYPTION_KEY` is
  unset, then runs only the e2e tests (failsafe against the shaded jar, as today)
- `lint` — `mvn fmt:check` (+ any static analysis kept from today's build)
- `clean` — `mvn clean`
- `proto` — regenerates the `../proto` classes (phase 2)

No WASM build steps, no stamps, no module-cache wipes — the module comes from the CDN at runtime.
The root `Makefile` adds `java` to `PROVIDERS` (which gains `test-e2e` passthrough per the
contract).

## Dockerfile

Stages `openfeature-provider-java.{base,build,test,lint,test_e2e}`, mirroring the Go block:

- `base`: `maven:3.9-eclipse-temurin-17` (+ `make`), `WORKDIR /src/openfeature-provider/java`,
  copy `pom.xml`, resolve dependencies (BuildKit cache mount on `/root/.m2` for layer speed),
  copy `../testdata/` to `/src/openfeature-provider/testdata/` (tests read it by relative path),
  then sources.
- `build` → `test` / `lint` / `test_e2e` as the contract; `test_e2e` mounts the two secrets and
  never writes them to a layer.
- Wire `test`+`lint` markers into `all`, `test_e2e` into `e2e`.

## Release

- `release-please-config.json` gains the java package (mirroring Go's entry): release-type
  `maven`, component `openfeature-provider/java`, tag-separator `/`, changelog
  `openfeature-provider/java/CHANGELOG.md`, extra-files `src/main/java/.../Version.java` if the
  version constant is kept (else pom only). The root config already sets `prerelease-type: rc`,
  so releases from `next` are `rc`s (first: `0.24.0-rc.1` from today's `0.23.4`). Add the manifest
  entry.
- Publishing keeps `central-publishing-maven-plugin` with secrets mounted as Docker secrets only.

## Docs

- `openfeature-provider/java/README.md`: rewritten as the flag-module provider (same shape as
  Go's): what a flag module is, the config table above, exposure modes, encrypted artifact +
  64-hex key, materialization stores (remote over HTTP, custom, default = unsupported fallback),
  `Resolve`/`ApplyFlags` with the unauthenticated-token warning, metrics, shutdown, "Track() not
  supported yet", release-candidate note pointing at `main` for the current release. Drop
  embedded-resolver, gRPC and state-fetcher sections; drop the stale 30s/15s poll-default drift —
  document 10s.
- `openfeature-provider/java/CLAUDE.md`: packages (public + internal map), the module ABI summary
  from `00-architecture.md`, gotchas (proto location `../proto`, generated-code dirs, failsafe
  against the shaded jar, sequential surefire, secrets-in-Docker rule).
- `STICKY_RESOLVE.md`: update to the engine-era materialization flow; the corrected
  default-store wording stays.
- Root `README.md` status table: Java → rewritten (matching Go's row).
- Delete `openfeature-provider/java/CONTRIBUTING.md` if it only describes the old WASM build
  steps; otherwise update.

## Post-release checklist (from the root README)

After the first `rc` ships, update the internal version registries with the new version.

## Done when

`make` at the repo root builds, tests and lints Java alongside Go; `docker build .` is green; a
release-please run would cut `0.24.0-rc.1`; and the root README lists Java as rewritten.
