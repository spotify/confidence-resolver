# Confidence Resolver — Core Rust Library

## Overview

The `confidence_resolver` crate implements feature flag resolution: parsing resolver state, evaluating targeting rules, bucketing users into variants, and logging resolve/assign events. It compiles to both native Rust and WebAssembly.

## Cargo Features

```toml
[features]
default = ["std", "json"]
std = ["chrono/clock", "rust-crypto-wasm"]       # enables OS clock + crypto
json = ["serde", "serde_json", "pbjson", "pbjson-types"]  # enables JSON/serde support
```

When compiled for WASM (`default-features = false`), `std` and `json` are disabled — the crate uses `chrono` without clock and skips serde/pbjson code.

## Error Handling

The crate does NOT use `thiserror`. It has a custom error system:

- **`ErrorCode`** — A 48-bit error code (renders as 8-char base64url). Created via:
  - `ErrorCode::from_location()` — derives code from call site (`#[track_caller]`)
  - `ErrorCode::from_tag("module.feature.case")` — stable tag-based code
  - `module_err!(":subsystem.case")` macro — prefixes with `module_path!()`
- **`Fallible<T>`** — Alias for `Result<T, ErrorCode>`, used in internal APIs
- **`OrFailExt`** — Trait to convert `Option<T>` / `Result<T, E>` into `Fallible<T>` via `.or_fail()?`
- **`fail!()`** macro — Early-return with `Err(ErrorCode)`
- **`ResolveError`** — Enum for the resolve path:
  - `Internal(ErrorCode)` — low-level "should never happen" errors
  - `Message(String)` — descriptive errors for API boundaries
  - `MissingMaterializations` — flag needs materializations, triggers suspend
  - `MaterializationsUnsupported` — caller has no materialization support
  - `UnrecognizedRule` — unknown rule type encountered

## Proto Generation (build.rs)

The build script compiles protos from `protos/`. Beyond standard prost generation:
- When `json` feature is enabled, `pbjson-build` generates additional `.serde.rs` files for each module
- Google well-known types are re-exported from `pbjson_types` (with `json`) or `prost_types` (without)
- It also generates `telemetry_config.rs` by scanning proto annotations for histogram configs and emitting `HistogramConfig` trait impls

## Key Types

- **`ResolverState`** — Parsed resolver state, created from proto via `ResolverState::from_proto()`
- **`ResolvedValue`** — Result of resolving a single flag
- **`Client`** — Client info extracted from resolver state
- **`Host`** trait — Abstraction for host-provided capabilities (logging, time, encryption)
- **`ResolveProcessState`** — Continuation state for multi-step resolve (materializations)

## Strict Clippy Lints

Outside of tests, the crate denies: `clippy::panic`, `clippy::unwrap_used`, `clippy::expect_used`, `clippy::indexing_slicing`, `clippy::arithmetic_side_effects`.

## Build & Test

```bash
make test
make lint
```

There is no `make build` target — this crate is a library, not a standalone binary.
