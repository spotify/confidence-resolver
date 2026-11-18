# Go OpenFeature Provider

## Overview

Go module: `github.com/spotify/confidence-resolver/openfeature-provider/go`

OpenFeature provider that resolves flags locally in a state-to-wasm **flag module**: a WebAssembly
module compiled per client credential from that client's flags, fetched encrypted from the
Confidence CDN and run on wazero (pure Go, no CGo).

## Architecture

- **`confidence/`** — public API: `NewProvider(ctx, ProviderConfig)`, the OpenFeature evaluation
  methods, `Resolve` / `ApplyFlags`, `MaterializationStore`, `ExposureMode` and the
  per-evaluation `_confidence_skip_apply` context key.
- **`internal/host`** — runs a module on wazero: validation, a capped shared memory, a pool of
  instances, resolve/evaluate/apply/flush calls, the host imports (`write_logs`, `current_time`,
  materialization reads/writes), and the dedupe filters, linked to the module wasm to wasm.
- **`internal/cdn`** — fetches and decrypts the credential's module (AES-GCM), with ETags.
- **`internal/ingest`** — ships flag logs to `ingestion.confidence.dev/v1/flagLogs:write`, with
  the module's log token as Bearer.
- **`internal/engine`** — module generations: polls the CDN, swaps in a new module without
  dropping evaluations, flushes logs on a timer and on swap and shutdown.
- **`internal/s2wpb`** — generated from `../proto` (`make proto`); don't edit.
- **`internal/testutil`** — a fake CDN and ingestion server for tests (`testutil.Backend`).

Test modules come from `../testdata/modules` (see its README). Event tracking (`Track()`) isn't
supported yet; it will come through the module.

## Build & Test

```bash
make build
make test       # unit tests
make test-e2e   # needs CONFIDENCE_CLIENT_SECRET and CONFIDENCE_CLIENT_ENCRYPTION_KEY
make lint
make proto      # regenerate internal/s2wpb
```

E2E tests are named `TestE2E_*`: `make test` skips them, `make test-e2e` runs only them, against
the production CDN and ingestion.

## Gotchas

- wazero's JIT and Go's profiling signals: see `UseWasmInterpreter` in the README.
- The race detector reports false positives on the shared memory's atomics; `internal/host`
  annotates them (`race.go`).
- A module's declared memory max is 4 GiB; the host caps it (`MaxMemoryBytes`, default 64 MiB),
  and the capped memory is reserved up front, so it shows as heap.
