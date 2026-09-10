# Go OpenFeature Provider

## Overview

Go module: `github.com/spotify/confidence-resolver/openfeature-provider/go`

Go OpenFeature provider using the Confidence resolver compiled to WASM, loaded via wazero (pure Go WASM runtime, no CGo).

## Architecture

- **WASM runtime**: wazero — pure Go, zero dependencies, no CGo
- **WASM embedding**: Resolver and event-engine WASM binaries are embedded at compile time via `//go:embed`
- **Pool architecture**: Resolver WASM instances run in a pool (`pool.go`) with a default size of 2, capped at `GOMAXPROCS`. Each instance is mutex-protected.
- **Crash recovery**: `recover.go` wraps resolvers with automatic WASM instance reload on panic/trap, preserving state and buffering logs through crashes.
- **Destination-aware flag logs**: Resolver state selects gRPC delivery to Spotify Edge or HTTP delivery to the Cloudflare ingestor, with ordered fallback.
- **Event tracking**: OpenFeature `Track` calls are batched by `confidence_event_engine.wasm` and published to the events service over gRPC.

## Key API

- **`NewProvider(ctx, ProviderConfig)`** (`provider_builder.go`) — Main factory function. Creates gRPC connection, state fetcher, flag logger, and wires everything together.
- **`NewProviderForTest(ctx, ProviderTestConfig)`** — Factory with injectable `StateProvider` and `FlagLogger` for testing.
- **`ProviderConfig`** — `ClientSecret`, `EncryptionKey`, `Logger`, `TransportHooks`, `MaterializationStore`, `UseRemoteMaterializationStore`, `StatePollInterval`, `LogPollInterval`, `ResolverPoolSize`, `UseWasmInterpreter`, `EnableApplyDedup`, `DisableExposureCollection`
- **`TransportHooks`** interface — Allows customizing both gRPC and HTTP transports (for proxying or testing): `ModifyGRPCDial(target, opts)` and `WrapHTTP(transport)`.

## Build & Test

```bash
make build
make test
make lint
make proto
```

## Background Goroutines

The provider starts background goroutines on `Init()`:
1. **State polling** — Fetches resolver state from CDN at `StatePollInterval` (default 10s)
2. **Log flushing** — Flushes resolve + assign logs at `LogPollInterval` (default 15s)
3. **Event flushing** — Flushes tracked events at `LogPollInterval`

All are cancelled via context on `Shutdown()`; pending events are drained before the event tracker closes.

## WASM Build

The resolver WASM is automatically built from source and copied to `confidence/internal/local_resolver/assets/` when building locally (skipped in Docker where it is provided by the build stage). The committed resolver and event-engine binaries are synchronized reproducibly with `make sync-wasm-go` and `make sync-wasm-event-go` from the repository root.
