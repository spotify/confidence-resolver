# Confidence Lambda Resolver

## Overview

Crate: `confidence-lambda-resolver` (internal, `publish = false`)

An AWS Lambda function that serves the Confidence flag resolver. Compiles `confidence_resolver` as a **native Rust binary** targeting ARM/Graviton (`aarch64-unknown-linux-gnu`). Mirrors the Cloudflare Worker architecture with AWS equivalents.

## Architecture

- **Compile-time state** -- Resolver state is embedded at build time via `include_bytes!("../../data/resolver_state_current.pb")`. Redeployment required to update.
- **Two Lambda functions from one crate** -- `resolver` (HTTP handler via Function URL) and `consumer` (SQS batch processor).
- **SQS-based log shipping** -- Flag logs are serialized to JSON and sent to SQS, then consumed by the consumer Lambda which aggregates and ships them. Equivalent to Cloudflare Queues.
- **DynamoDB metrics** -- Cumulative `TelemetrySnapshot` stored in DynamoDB, exposed via `/metrics` endpoint. Equivalent to Cloudflare KV.
- **DynamoDB materializations** -- Optional sticky assignment storage. Equivalent to planned Cloudflare KV/DO materializations.
- **JSON API** -- Same JSON request/response format as the Cloudflare resolver.
- **CORS** -- All responses include CORS headers with configurable `ALLOWED_ORIGIN`.

## Endpoints (resolver Lambda)

| Method | Path | Description |
|--------|------|-------------|
| POST | `/v1/flags:resolve` | Resolve flags (JSON body, `apply` defaults to `true`) |
| POST | `/v1/flags:apply` | Apply flags (JSON body) |
| GET | `/v1/state:etag` | Returns deployment state ETag and resolver version |
| GET | `/metrics` | Prometheus metrics (requires ClientSecret auth) |
| OPTIONS | `*` | CORS preflight |

## Consumer Lambda

Triggered by SQS (batch config: max 100 messages, 10s window):
1. Deserializes each message from JSON to `WriteFlagLogsRequest`
2. Aggregates the batch via `flag_logger::aggregate_batch`
3. Accumulates telemetry in DynamoDB (read-modify-write)
4. Ships aggregated logs to the Confidence API

## Environment Variables

| Variable | Binary | Description |
|----------|--------|-------------|
| `CONFIDENCE_CLIENT_SECRET` | Both | Client secret for API authentication |
| `ALLOWED_ORIGIN` | resolver | CORS allowed origin (defaults to `"*"`) |
| `RESOLVER_STATE_ETAG` | resolver | ETag of the embedded resolver state |
| `DEPLOYER_VERSION` | resolver | Version of confidence-resolver used for deployment |
| `SQS_QUEUE_URL` | resolver | SQS queue URL for flag log shipping |
| `DYNAMODB_METRICS_TABLE` | Both | DynamoDB table name for Prometheus metrics |
| `DYNAMODB_MATERIALIZATIONS_TABLE` | resolver | DynamoDB table name for sticky assignments |

## Build & Test

```bash
make build   # cross-compile for aarch64-unknown-linux-gnu
make lint    # clippy
```

## Deployer

The `deployer/` directory contains a deployment script. See `deployer/README.md`.

## Key Differences from Cloudflare Resolver

1. **Native binary** -- Compiles to `aarch64-unknown-linux-gnu`, not WASM
2. **SQS** -- Uses SQS instead of Cloudflare Queues for log shipping
3. **DynamoDB** -- Uses DynamoDB instead of Cloudflare KV for metrics and materializations
4. **Two binaries** -- Separate resolver and consumer Lambda functions instead of one Worker with fetch + queue handlers
5. **`std::time::Instant`** -- No `scheduler.wait(0)` hack needed for timing
6. **Materialization support** -- DynamoDB-backed suspend/resume protocol (Cloudflare version planned but not yet implemented)
