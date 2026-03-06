# Python OpenFeature Provider

## Overview

PyPI package: `confidence-openfeature-provider`

Python OpenFeature provider using the Confidence resolver compiled to WASM, loaded via wasmtime.

## Architecture

- **wasmtime** — WASM runtime
- **Crash recovery** — `LocalResolver` wraps `WasmResolver` with automatic WASM instance reload on `RuntimeError` or `wasmtime.Trap`, caching state for recovery and buffering logs through crashes
- **WASM loading** — Binary loaded from package resources via `importlib.resources` (Python 3.9+) with `pkg_resources` fallback
- **Threading** — Background threads for state polling and log flushing (not asyncio)
- **gRPC** — Flag logs shipped via `grpcio`
- **httpx** — HTTP client for state fetching from CDN

## Background Threads

The provider starts background threads:
1. **State polling** — Fetches resolver state from CDN (default 30s)
2. **Log flushing** — Flushes resolve + assign logs via gRPC (default 15s)
3. **Assign flushing** — Flushes assign logs at higher frequency (default 100ms)

## Build & Test

```bash
make build      # build WASM + create venv + install + python -m build
make test       # pytest tests/ (excludes e2e)
make test-e2e   # pytest e2e tests
make lint       # ruff check + ruff format --check + mypy
make format     # ruff check --fix + ruff format
make proto      # generate protobuf Python files from ../proto/
make install    # create venv + pip install -e ".[dev]"
```

## Gotchas

- **WASM packaging**: The `hatchling` build system includes the WASM binary in the wheel via `force-include` from `resources/wasm/`. Locally, WASM is built from source and copied there. In Docker, it's provided by the build stage.
- **Proto location**: Generated from `../proto/` (i.e., `openfeature-provider/proto/`), output goes to `src/confidence/proto/`.
