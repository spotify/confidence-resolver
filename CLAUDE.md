# Confidence Resolver - Development Guide

## Repository Overview

Multi-language workspace implementing feature flag resolution in Rust, compiled to WebAssembly with bindings for JS, Java, Go, Ruby, Rust, and Python.

**Repository**: `spotify/confidence-resolver`

### Key Components

- **confidence-resolver/** — Core Rust resolver library (flag evaluation, targeting, bucketing)
- **wasm/rust-guest/** — WASM guest (compiles the resolver to `wasm32-unknown-unknown`)
- **wasm-msg/** — WASM messaging layer (alloc/free, protobuf-based host↔guest calls)
- **confidence-cloudflare-resolver/** — Cloudflare Worker WASM build
- **openfeature-provider/js/** — TypeScript OpenFeature provider (npm: `@spotify-confidence/openfeature-server-provider-local`)
- **openfeature-provider/java/** — Java OpenFeature provider (Maven Central: `com.spotify.confidence:openfeature-provider-local`)
- **openfeature-provider/go/** — Go OpenFeature provider
- **openfeature-provider/ruby/** — Ruby OpenFeature provider (**online/remote resolver, NOT WASM**)
- **openfeature-provider/rust/** — Rust OpenFeature provider (**native resolver, no WASM**)
- **openfeature-provider/python/** — Python OpenFeature provider
- **openfeature-provider/proto/** — Shared protobuf definitions used by all providers
- **mock-support-server/** — Go mock server for integration/benchmark testing

## Cargo Workspace Gotcha

Several workspace members are **dummy Cargo crates** that exist solely for Release Please versioning (java, js, go, python). `default-members` excludes them, so `cargo build` only builds real Rust crates. Don't be confused by Cargo.toml files in non-Rust provider directories.

## WASM Architecture

```
Host (JS/Java/Go/Python/Ruby)
    ↓ protobuf message via wasm-msg (alloc → write → call → read → free)
WASM Guest (rust-guest, compiled from confidence-resolver)
    ↓ returns protobuf response
Host
```

- `wasm-msg` provides memory management (`wasm_msg_alloc`/`wasm_msg_free`) and the `wasm_msg_guest!`/`wasm_msg_host!` macros
- Guest exports are prefixed: `wasm_msg_guest_resolve_flags`, `wasm_msg_guest_set_resolver_state`, etc.
- Host imports are prefixed: `wasm_msg_host_log_message`, `wasm_msg_host_current_time`

### Building and inspecting the WASM

```bash
# Build locally (requires Rust + wasm32-unknown-unknown target)
make wasm/confidence_resolver.wasm

# Or extract from Docker (no local Rust needed)
docker build --target wasm-rust-guest.artifact -o wasm .
# produces: wasm/confidence_resolver.wasm

# Install wabt (WebAssembly Binary Toolkit) for inspection tools
brew install wabt            # macOS
apt-get install wabt         # Debian/Ubuntu

# Inspect exports
wasm-objdump -j Export -x wasm/confidence_resolver.wasm

# Inspect host imports
wasm-objdump -j Import -x wasm/confidence_resolver.wasm
```

## WASM Rebuild: Go Provider

The Go provider embeds the WASM binary via `//go:embed`, so the committed `.wasm` file must be kept in sync. **After any changes to `confidence-resolver/`, `wasm-msg/`, or `wasm/rust-guest/`**, run:

```bash
make sync-wasm-go
```

This builds the WASM in Docker and copies it to `openfeature-provider/go/confidence/internal/local_resolver/assets/`. The updated `.wasm` file must be committed.

## Protobuf Schema Locations

There are 4 separate proto directories — this is the most common source of confusion:

- **`confidence-resolver/protos/`** — Core resolver protos (flags, admin, resolver API, types, events)
- **`openfeature-provider/proto/`** — Shared provider protos (WASM messages, flag types) — used by JS, Java, Go, Ruby, Python providers
- **`wasm/proto/`** — WASM guest message definitions (`messages.proto`, `types.proto`)
- **`wasm-msg/proto/`** — Low-level messaging layer protos

## Publishing & Security

### Critical: Secrets Must Never Be Written to Docker Layers

```dockerfile
# CORRECT — secret mounted, never persisted
RUN --mount=type=secret,id=my_secret,target=/path/to/secret \
    command-that-uses /path/to/secret

# WRONG — secret written to layer
RUN --mount=type=secret,id=my_secret \
    cat /run/secrets/my_secret > config.file
```

- **JS** — Build in Docker (`npm pack`), publish via GitHub Actions OIDC (no npm tokens). Requires npm Trusted Publishers config.
- **Java** — Credentials mounted as Docker secrets. Requires GitHub secrets: `MAVEN_SETTINGS`, `GPG_PRIVATE_KEY`, `SIGN_KEY_PASS`. Uses `central-publishing-maven-plugin` (not nexus-staging).
- **Rust** — Published via Docker stages to crates.io.

## Post-Release Checklist

After releasing a new version of any SDK from this repo, update internal version registries with the new version numbers.

## Build & Run

### Root Makefile

```bash
make                # lint + test + build everything
make test           # run all component tests
make lint           # run all linters
make build          # build WASM + all provider packages
make wasm/confidence_resolver.wasm  # build WASM artifact only
make sync-wasm-go   # build WASM in Docker and sync to Go assets (for committing)
make go-bench       # Go benchmark via docker-compose
make js-bench       # JS benchmark via docker-compose
```

Each component has its own Makefile with `build`, `test`, `lint`, `clean` targets.

### Docker

The Dockerfile uses multi-stage builds. Every component has `.<action>` stages (e.g., `openfeature-provider-js.test`):

```bash
docker build .                                              # validate everything
docker build --target wasm-rust-guest.artifact -o wasm .    # extract .wasm file
docker build --target openfeature-provider-js.artifact -o artifacts .  # extract JS tarball
docker build --target openfeature-provider-js.test .        # run JS tests
docker build --target openfeature-provider-java.build .     # build Java provider
```

Stage naming pattern: `<component>.{build,test,test_e2e,lint,artifact,publish}`

## Environment Variables

| Variable | Purpose |
|----------|---------|
| `IN_DOCKER_BUILD=1` | Set in Docker stages — Makefiles skip WASM rebuild when set |
| `DEBUG=cnfd:*` | Enable debug logging in JS provider |
