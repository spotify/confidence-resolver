# Go OpenFeature Provider

## Overview

Go module: `github.com/spotify/confidence-resolver/openfeature-provider/go`

Go OpenFeature provider using the Confidence resolver compiled to WASM, loaded via wazero (pure Go WASM runtime, no CGo).

## Architecture

- **WASM runtime**: wazero — pure Go, zero dependencies, no CGo
- **WASM embedding**: The WASM binary is embedded at compile time via `//go:embed` in `confidence/internal/local_resolver/wasm.go`
- **Pool architecture**: Multiple WASM instances run in a pool (`pool.go`) sized to `GOMAXPROCS` by default. Each instance is mutex-protected.
- **Crash recovery**: `recover.go` wraps resolvers with automatic WASM instance reload on panic/trap, preserving state and buffering logs through crashes.
- **gRPC**: Flag logs are shipped via gRPC to `edge-grpc.spotify.com` using `InternalFlagLoggerServiceClient`.

## Key API

- **`NewProvider(ctx, ProviderConfig)`** (`provider_builder.go`) — Main factory function. Creates gRPC connection, state fetcher, flag logger, and wires everything together.
- **`NewProviderForTest(ctx, ProviderTestConfig)`** — Factory with injectable `StateProvider` and `FlagLogger` for testing.
- **`ProviderConfig`** — `ClientSecret`, `Logger`, `TransportHooks`, `MaterializationStore`, `UseRemoteMaterializationStore`, `StatePollInterval`, `LogPollInterval`, `ResolverPoolSize`
- **`TransportHooks`** interface — Allows customizing both gRPC and HTTP transports (for proxying or testing): `ModifyGRPCDial(target, opts)` and `WrapHTTP(transport)`.

## Build & Test

```bash
make build  # build WASM (if not in Docker) + go build ./...
make test   # build + go test -v ./...
make lint   # gofmt check (excluding proto/) + go vet
make proto  # run scripts/generate_proto.sh
```

## Background Goroutines

The provider starts background goroutines on `Init()`:
1. **State polling** — Fetches resolver state from CDN at `StatePollInterval` (default 10s)
2. **Log flushing** — Flushes resolve + assign logs via gRPC at `LogPollInterval` (default 15s)

Both are cancelled via context on `Shutdown()`.

## WASM Build

WASM is automatically built from source and copied to `confidence/internal/local_resolver/assets/` when building locally (skipped in Docker where it's provided by the build stage).
