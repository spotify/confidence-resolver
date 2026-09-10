# Python OpenFeature Provider

## Overview

PyPI package: `confidence-openfeature-provider`

Python OpenFeature provider using Confidence resolver and event-engine WASM modules, loaded via wasmtime.

## Architecture

- **wasmtime** — WASM runtime
- **Crash recovery** — `LocalResolver` wraps `WasmResolver` with automatic WASM instance reload on `RuntimeError` or `wasmtime.Trap`, caching state for recovery and buffering logs through crashes
- **WASM loading** — Both binaries are loaded from package resources via `importlib.resources`, with compatibility fallbacks
- **Threading** — Background threads for state polling and log flushing (not asyncio)
- **Destination-aware flag logs** — Resolver state selects gRPC or Cloudflare HTTP delivery with fallback
- **Event tracking** — OpenFeature `track()` calls are batched in `confidence_event_engine.wasm` and published over gRPC
- **httpx** — HTTP client for state fetching from CDN

## Background Threads

The provider starts two long-lived background threads:
1. **State polling** — Fetches resolver state from CDN (default 30s)
2. **Flush loop** — Flushes resolve logs and tracked events every 15s, and assign logs every 100ms

Event publishing uses a small thread-pool executor so gRPC publishing does not block the flush loop.

## Provider Options

In addition to polling and materialization settings, `ConfidenceProvider` accepts an optional AES-256 state `encryption_key`, experimental `enable_apply_dedup`, and `disable_exposure_collection`.

## Build & Test

```bash
make build      # build both WASM resources + create venv + install + python -m build
make test       # pytest tests/ (excludes e2e)
make test-e2e   # pytest e2e tests
make lint       # ruff check + ruff format --check + mypy
make format     # ruff check --fix + ruff format
make proto      # generate protobuf Python files from ../proto/
make install    # create venv + pip install -e ".[dev]"
```

## Gotchas

- **WASM packaging**: The `hatchling` build system includes both WASM binaries in the wheel via `force-include` from `resources/wasm/`. Locally, they are built from source and copied there. In Docker, they are provided by build stages.
- **Proto location**: Generated from `../proto/` (i.e., `openfeature-provider/proto/`), output goes to `src/confidence/proto/`.
