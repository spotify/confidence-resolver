# Rust OpenFeature Provider

## Overview

Crate: `spotify-confidence-openfeature-provider-local` (published to crates.io)

Rust OpenFeature provider for Confidence. **Uses the `confidence_resolver` crate directly** — native Rust, no WASM indirection. The resolver runs as native Rust code with all features enabled (`std`, `json`), opposite of the WASM guest which disables them.

## Architecture

- **Native resolver** — Links directly to `confidence_resolver` crate
- **Async** — Built on `tokio` with background tasks for state polling and log flushing
- **`reqwest`** — HTTP client for state fetching from CDN and log shipping
- **Builder pattern** — `ProviderOptions::new(secret).with_*()` chain for configuration
- **`gateway_url`** option — Routes all HTTP requests through a proxy, preserving original host in `X-Forwarded-Host`

## Background Tasks

The provider spawns tokio tasks on initialization:
1. **State polling** — Fetches resolver state from CDN (default 30s)
2. **Log flushing** — Flushes resolve logs via HTTP (default 15s)
3. **Assign flushing** — Flushes assign logs at higher frequency (default 100ms)

Shutdown sends a oneshot signal and awaits all background tasks.

## Build & Test

```bash
make build      # cargo build
make test       # cargo test --lib
make test-e2e   # cargo test --test e2e
make lint       # cargo fmt --check + cargo clippy -- -D warnings
```

## Proto Generation

`build.rs` compiles protos from `../../confidence-resolver/protos/` — specifically `internal_api.proto` for the remote materialization API. Generated code is included as `remote_proto` module.
