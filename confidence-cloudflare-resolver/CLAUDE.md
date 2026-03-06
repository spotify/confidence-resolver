# Confidence Cloudflare Resolver

## Overview

Crate: `confidence-cloudflare-resolver` (internal, `publish = false`)

A Cloudflare Worker that serves the Confidence flag resolver at the edge. Compiles `confidence_resolver` and a pre compiled confidence "state" directly into a Cloudflare Worker WASM module (`wasm32-unknown-unknown` + `worker` crate).

## Architecture

- **Compile-time state** — Resolver state is embedded at build time via `include_bytes!("../../data/resolver_state_current.pb")`. No runtime state fetching. Redeployment required to update.
- **No sticky assignments support** — Uses `ResolveProcessRequest::without_materializations()`.
- **Queue-based log shipping** — Flag logs are serialized to JSON and sent to a Cloudflare Queue (`flag_logs_queue`), then consumed by a queue handler that aggregates and ships them.
- **JSON API** — Request/response bodies are JSON (not protobuf), unlike the WASM-based providers.
- **CORS** — All responses include CORS headers with configurable `ALLOWED_ORIGIN`.

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| POST | `/v1/flags:resolve` | Resolve flags (JSON body, `apply` defaults to `true`) |
| POST | `/v1/flags:apply` | Apply flags (JSON body) |
| GET | `/v1/state:etag` | Returns deployment state ETag and resolver version |
| OPTIONS | `*` | CORS preflight |

Note: The Worker router uses `*path` matching because `:name` conflicts with Cloudflare router parameter syntax.

## Queue Consumer

The `#[event(queue)]` handler `consume_flag_logs_queue` processes batched flag log messages:
1. Deserializes each message from JSON to `WriteFlagLogsRequest`
2. Aggregates the batch via `flag_logger::aggregate_batch`
3. Ships aggregated logs to the Confidence API

## Environment Variables

| Variable | Description |
|----------|-------------|
| `CONFIDENCE_CLIENT_SECRET` | Client secret for API authentication |
| `ALLOWED_ORIGIN` | CORS allowed origin (defaults to `"*"`) |
| `RESOLVER_STATE_ETAG` | ETag of the embedded resolver state |
| `DEPLOYER_VERSION` | Version of confidence-resolver used for deployment |

## Build & Test

```bash
make build
make lint
```

Deployment is handled by `deployer/Dockerfile` — see `deployer/README.md`.

## Build Gotcha

The `getrandom_backend="wasm_js"` RUSTFLAGS cfg is **required** — without it, the `getrandom` crate will fail to compile for the Cloudflare Worker environment:

```bash
RUSTFLAGS='--cfg getrandom_backend="wasm_js"' cargo build --target wasm32-unknown-unknown --release
```

## Compile-Time Data

Two files are embedded at compile time from `../../data/`:
- `resolver_state_current.pb` — Protobuf-encoded `SetResolverStateRequest` containing resolver state + account ID
- `encryption_key` — Base64-encoded encryption key for resolve tokens

## Deployer

The `deployer/` directory contains a deployment script and Dockerfile for automated deployment. See `deployer/README.md`.

## Key Differences from Other Providers

1. **No runtime state updates** — State is compile-time, not polled from CDN
2. **No materializations** — Sticky assignments not supported
3. **JSON, not protobuf** — API uses JSON request/response bodies
4. **Edge deployment** — Runs on Cloudflare's edge network, not in application processes
5. **Queue logging** — Uses Cloudflare Queues instead of gRPC for log transport
