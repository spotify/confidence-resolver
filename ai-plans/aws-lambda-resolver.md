# AWS Lambda Resolver — Implementation Plan

## Context

The Confidence resolver runs on Cloudflare Workers at the edge via `confidence-cloudflare-resolver/`. Users also need AWS Lambda support for teams whose infrastructure is on AWS. The Lambda resolver mirrors the Cloudflare architecture 1:1: compile-time state embedding, JSON API, queue-based log batching, KV-equivalent metrics store, and a deployer-based deployment lifecycle. It compiles the core `confidence_resolver` crate as a **native Rust binary** (not WASM) targeting ARM/Graviton.

## Architecture

```
                         Cloudflare                              AWS Lambda
                         ─────────                               ──────────
HTTP endpoint            Workers URL                             Lambda Function URL
Resolver runtime         WASM (wasm32-unknown-unknown)           Native (aarch64-unknown-linux-gnu)
State model              include_bytes! (compile-time)           include_bytes! (compile-time)
Event queue              Cloudflare Queues                       SQS
Queue consumer           #[event(queue)] in same Worker          Separate consumer Lambda (same binary)
Metrics store            Cloudflare KV                           DynamoDB (metrics table)
Metrics endpoint         GET /metrics (reads KV)                 GET /metrics (reads DynamoDB)
Materialization store    KV / Durable Objects (planned)          DynamoDB (materializations table)
Materialization protocol suspend/resume (planned)                suspend/resume
Log backend              POST resolver.confidence.dev            POST resolver.confidence.dev
Deployer                 Docker + Wrangler CLI                   Docker + AWS CLI
```

### Request flow

```
Client
  │
  ▼
Resolver Lambda (Function URL)
  ├── POST /v1/flags:resolve  →  resolve flags, return JSON response
  ├── POST /v1/flags:apply    →  apply flags, return empty response
  ├── GET  /v1/state:etag     →  return deployment state
  ├── GET  /metrics            →  read DynamoDB, return Prometheus text
  └── After response: send WriteFlagLogsRequest JSON to SQS
                │
                ▼
         SQS Queue (batches up to 100 messages, 10s window)
                │
                ▼
Consumer Lambda (SQS trigger)
  ├── Deserialize batch → Vec<WriteFlagLogsRequest>
  ├── Aggregate via flag_logger::aggregate_batch()
  ├── Accumulate telemetry in DynamoDB (read-modify-write)
  └── Ship aggregated logs to resolver.confidence.dev/v1/clientFlagLogs:write
```

### Deployer lifecycle

```
Deployer (Docker container, runs on schedule)
  1. Validate credentials (AWS creds + CONFIDENCE_CLIENT_SECRET)
  2. Compute CDN URL from SHA256(client_secret)
  3. Check deployed ETag via Function URL /v1/state:etag
  4. Conditional fetch from CDN (If-None-Match → skip on 304)
  5. Write data/resolver_state_current.pb + data/encryption_key
  6. Create SQS queue if not exists
  7. Create DynamoDB table if not exists (when ENABLE_METRICS is set)
  8. Cross-compile Rust binary for aarch64-unknown-linux-gnu
  9. Package as Lambda zip (bootstrap binary)
  10. Deploy resolver Lambda + consumer Lambda via AWS CLI
  11. Ensure Function URL, SQS trigger, IAM roles
```

## New Files

### `confidence-lambda-resolver/`

```
confidence-lambda-resolver/
├── Cargo.toml
├── Makefile
├── CLAUDE.md
├── src/
│   ├── resolver.rs        # Resolver Lambda: HTTP handler, routing, Host impl
│   ├── consumer.rs        # Consumer Lambda: SQS batch processing, DynamoDB metrics
│   ├── materialization.rs # DynamoDB materialization store
│   └── common.rs          # Shared: state init, SQS client, DynamoDB client, CORS, log shipping
├── deployer/
│   ├── script.sh          # Deployment script (adapted from Cloudflare deployer)
│   └── README.md
```

### `Cargo.toml`

Two binaries from one crate — same pattern as having fetch + queue handlers in one Cloudflare Worker:

```toml
[[bin]]
name = "resolver"
path = "src/resolver.rs"

[[bin]]
name = "consumer"
path = "src/consumer.rs"
```

Dependencies:
- `confidence_resolver` (path dep, version 0.17.0, default features — `std` + `json`)
- `lambda_http` — HTTP event handling for resolver Lambda
- `lambda_runtime` — Lambda execution model
- `aws-sdk-sqs` — SQS message sending (resolver) and receiving metadata
- `aws-sdk-dynamodb` — DynamoDB read/write for metrics
- `aws-config` — AWS credential/region loading
- `tokio` (rt, macros) — async runtime
- `serde_json` — JSON request/response bodies
- `prost` — protobuf for state decoding
- `bytes` — byte buffer handling
- `reqwest` (rustls-tls) — log shipping to Confidence backend
- `tracing`, `tracing-subscriber` — structured logging to CloudWatch
- `base64` — encryption key decoding

### `src/resolver.rs` — Resolver Lambda handler

Mirrors `confidence-cloudflare-resolver/src/lib.rs` lines 137-374:

**State initialization** — same `include_bytes!` pattern:
```rust
const CDN_STATE_BYTES: &[u8] = include_bytes!("../../data/resolver_state_current.pb");
const ENCRYPTION_KEY_BASE64: &str = include_str!("../../data/encryption_key");
```
Parse into `static RESOLVER_STATE: LazyLock<ResolverState>` at cold start.

**Host trait** — `LambdaHost` struct implementing `Host`:
- `log()` → `tracing::debug!`
- `log_resolve()` → `resolve_logger::build_resolve_log()`, store in thread-local `FLAG_LOG: RefCell<Option<WriteFlagLogsRequest>>` (same pattern as Cloudflare `lib.rs:43-49`)
- `log_assign()` → `assign_logger::build_flag_assigned()`, store in thread-local
- `current_time()` and `encrypt_resolve_token()` — use `std` defaults

**Request routing** — match on `(method, path)`:
- `POST /v1/flags:resolve` → resolve with materialization support (see below)
- `POST /v1/flags:apply` → same logic as Cloudflare `lib.rs:325-353`
- `GET /v1/state:etag` → same logic as Cloudflare `lib.rs:201-212`
- `GET /metrics` → read `prometheus` item from DynamoDB, return Prometheus text (same as Cloudflare `lib.rs:176-198` but DynamoDB instead of KV)
- `OPTIONS *` → CORS preflight
- Else → 404

**Materialization support (suspend/resume protocol)** — follows the same pattern as the Rust native provider (`provider.rs:299-468`):
1. If materialization store is configured → `ResolveProcessRequest::deferred_materializations(request)`
2. If not → `ResolveProcessRequest::without_materializations(request)`
3. Call `resolver.resolve_flags(initial_request)`
4. If `Suspended` → read materializations from store, build `ResolveProcessRequest::resume(records, state)`, resolve again
5. If `Resolved` → write any `materializations_to_write` to store (async, fire-and-forget)
6. A second `Suspended` is an error

**Materialization store** — `DynamoDbMaterializationStore` backed by a DynamoDB table (`CONFIDENCE_MATERIALIZATIONS_DYNAMODB`):
- Implements the same `MaterializationStore` read/write operations as the Rust native provider's `ConfidenceRemoteMaterializationStore`, but stored locally in DynamoDB instead of calling the remote API
- Key schema: partition key = `unit`, sort key = `materialization#rule`
- Attributes: `variant` (string), `included` (bool)
- Read: `BatchGetItem` for variant reads, `Query` for inclusion checks
- Write: `BatchWriteItem` for variant assignments
- Equivalent to the planned Cloudflare KV-backed store (`CONFIDENCE_MATERIALIZATIONS_KV`)
- Enabled via `ENABLE_STICKY_ASSIGNMENTS_DYNAMODB` env var (deployer creates the table)
- Alternative: `ENABLE_STICKY_ASSIGNMENTS_REMOTE` → use `ConfidenceRemoteMaterializationStore` (HTTP to `resolver.confidence.dev`)

**Log shipping to SQS** — after computing the response, send the accumulated `WriteFlagLogsRequest` as JSON to SQS. Equivalent to Cloudflare's `ctx.wait_until()` + `queue.send()` at `lib.rs:361-371`. On Lambda, use `tokio::spawn` to send asynchronously during the response phase.

**Latency measurement** — `std::time::Instant` instead of the `scheduler.wait(0)` hack (Cloudflare `lib.rs:291-313`). Feed into `telemetry::build_request_telemetry()`.

### `src/consumer.rs` — Consumer Lambda handler

Mirrors the Cloudflare `#[event(queue)]` handler at `lib.rs:376-446`:

- Triggered by SQS with batch config (max 100 messages, 10s window) — same as Cloudflare Queue consumer config in `wrangler.toml:8-11`
- Deserialize each SQS message body from JSON to `WriteFlagLogsRequest`
- Aggregate via `flag_logger::aggregate_batch(logs)` — reuses `lib.rs:391`
- Call `update_prometheus_dynamo()` — equivalent of `update_prometheus_kv()` at `lib.rs:410-431`:
  - Read cumulative `TelemetrySnapshot` from DynamoDB item (key: `"snapshot"`)
  - Call `cumulative.accumulate_delta(td)`
  - Write updated snapshot + rendered Prometheus text back to DynamoDB
  - Same race condition caveat as Cloudflare's KV approach
- Ship aggregated logs to `resolver.confidence.dev/v1/clientFlagLogs:write` via `reqwest` — equivalent of `send_flags_logs()` at `lib.rs:433-446`

### `src/materialization.rs` — DynamoDB Materialization Store

Implements the same `MaterializationStore` interface as the Rust native provider (`openfeature-provider/rust/src/materialization.rs`), backed by DynamoDB instead of the remote Confidence API:

- `DynamoDbMaterializationStore` struct wrapping an `aws_sdk_dynamodb::Client`
- `read_materializations()` → `BatchGetItem` for variant reads, `Query` for inclusion checks
- `write_materializations()` → `BatchWriteItem` for variant assignments
- Key design: partition key = `unit`, sort key = `materialization#rule`
- Conversion functions reuse the same `ReadOpType`/`ReadResultType`/`WriteOp` types from the Rust provider's `materialization.rs`

Equivalent to the planned Cloudflare KV-backed store. Uses DynamoDB's consistent reads for correctness.

### `src/common.rs` — Shared code

- State initialization (`CDN_STATE_BYTES`, `RESOLVER_STATE`, `ENCRYPTION_KEY`)
- AWS SDK clients (SQS, DynamoDB) initialized from env
- CORS helpers (`ResponseExt` trait, same as Cloudflare `lib.rs:448-461`)
- Materialization store initialization (DynamoDB vs Remote vs None, based on env vars)
- Environment variable reading (`CONFIDENCE_CLIENT_SECRET`, `ALLOWED_ORIGIN`, `RESOLVER_STATE_ETAG`, `DEPLOYER_VERSION`, `SQS_QUEUE_URL`, `DYNAMODB_METRICS_TABLE`, `DYNAMODB_MATERIALIZATIONS_TABLE`)

### `deployer/script.sh`

Adapted from `confidence-cloudflare-resolver/deployer/script.sh` (543 lines). Same lifecycle, AWS commands instead of Cloudflare API/Wrangler:

| Cloudflare deployer step | AWS Lambda deployer equivalent |
|--------------------------|-------------------------------|
| Validate `CLOUDFLARE_API_TOKEN` | Validate AWS credentials (STS `get-caller-identity`) |
| Auto-detect Cloudflare Account ID | Use `AWS_REGION` / auto-detect from config |
| Build Workers subdomain URL | Build Function URL from Lambda ARN |
| Create Cloudflare Queue | `aws sqs create-queue` (flag-logs-queue) |
| Create KV namespace (metrics) | `aws dynamodb create-table` (resolver-metrics) |
| Create KV namespace (materializations) | `aws dynamodb create-table` (resolver-materializations) |
| `worker-build --release` | `cargo build --release --target aarch64-unknown-linux-gnu` |
| `wrangler deploy` | `aws lambda update-function-code` + `update-function-configuration` |
| Set wrangler.toml vars | Set Lambda env vars via `update-function-configuration` |

**Required env vars**: `AWS_REGION`, `CONFIDENCE_CLIENT_SECRET`
**Optional env vars**: `LAMBDA_FUNCTION_NAME` (default: `confidence-lambda-resolver`), `FUNCTION_NAME_PREFIX`, `LAMBDA_MEMORY_MB` (default: 128), `LAMBDA_TIMEOUT` (default: 10), `LAMBDA_ROLE_ARN`, `ALLOWED_ORIGIN`, `RESOLVE_TOKEN_ENCRYPTION_KEY`, `ENABLE_METRICS`, `ENABLE_STICKY_ASSIGNMENTS_DYNAMODB`, `ENABLE_STICKY_ASSIGNMENTS_REMOTE`, `FORCE_DEPLOY`, `NO_DEPLOY`

### Dockerfile stages

Added to root `Dockerfile`, mirroring the Cloudflare stages at lines 224-258:

- **`confidence-lambda-resolver.build`** — FROM `rust-deps`. Add `aarch64-unknown-linux-gnu` target + cross-linker. Copy source + `data/`. Build both binaries.
- **`confidence-lambda-resolver.lint`** — FROM build stage, run clippy.
- **`confidence-lambda-resolver.artifact`** — Extract both `bootstrap` binaries as zips.
- **`confidence-lambda-resolver.deployer`** — FROM build stage, install AWS CLI v2, jq, bash. Clean `data/`. CMD → `deployer/script.sh`.

### Root Makefile additions

```makefile
build-lambda-deployer:
    docker build --target confidence-lambda-resolver.deployer \
        --build-arg COMMIT_SHA=$(shell git rev-parse HEAD) \
        -t confidence-lambda-deployer:latest .
```

### Workspace Cargo.toml

Add `confidence-lambda-resolver` to workspace members (but **not** to `default-members`).

## Reuse vs New

### Reused from existing infrastructure (no changes needed)

| Component | Source | How it's reused |
|-----------|--------|-----------------|
| Core resolver | `confidence-resolver/` crate | Same path dependency, same `ResolverState`, `Host`, `AccountResolver` API |
| State CDN | `confidence-resolver-state-cdn.spotifycdn.com` | Same CDN, same URL derivation (`SHA256(client_secret)`) |
| `data/` convention | `data/resolver_state_current.pb`, `data/encryption_key` | Same `include_bytes!` / `include_str!` pattern |
| Proto types + JSON | `pbjson` serde impls on all request/response types | Same JSON API contract as Cloudflare |
| Telemetry helpers | `telemetry::build_request_telemetry()`, `TelemetrySnapshot` | Same per-request telemetry construction |
| Telemetry accumulation | `TelemetrySnapshot::accumulate_delta()`, `to_prometheus()` | Same read-modify-write pattern, DynamoDB instead of KV |
| Log construction | `resolve_logger::build_resolve_log()`, `assign_logger::build_flag_assigned()` | Same stateless log builders |
| Log batching | `flag_logger::aggregate_batch()` | Same batch aggregation in consumer Lambda |
| Log backend | `resolver.confidence.dev/v1/clientFlagLogs:write` | Same backend endpoint |
| Materialization protocol | `ResolveProcessRequest::deferred_materializations()`, `resume()` | Same suspend/resume protocol as native Rust provider |
| Materialization types | `ReadOpType`, `ReadResultType`, `WriteOp`, conversion functions | Reuse from `openfeature-provider/rust/src/materialization.rs` |
| Remote materialization store | `ConfidenceRemoteMaterializationStore` | Available as fallback when DynamoDB is not configured |
| Dockerfile dep cache | `rust-deps` stage | Reuse the shared dependency pre-build stage |
| Deployer logic | `deployer/script.sh` patterns | CDN fetch, ETag check, conditional deploy — same algorithm |

### Must be built new (Lambda-specific)

| Component | Why it can't be shared | Cloudflare equivalent |
|-----------|----------------------|----------------------|
| **`resolver.rs`** | `lambda_http` types differ from `worker` crate | `lib.rs` fetch handler (lines 137-374) |
| **`consumer.rs`** | SQS event format differs from Cloudflare Queue `MessageBatch` | `lib.rs` queue consumer (lines 376-446) |
| **`materialization.rs`** | DynamoDB API vs Cloudflare KV API | Planned KV-backed store (not yet implemented) |
| **`common.rs`** | AWS SDK clients (SQS, DynamoDB) vs Cloudflare bindings (Queue, KV) | Scattered across `lib.rs` statics |
| **`LambdaHost` struct** | `tracing` instead of `console_log!`, `Instant` instead of `js_sys::Date` | `H` struct in `lib.rs` |
| **DynamoDB metrics** | `update_prometheus_dynamo()` using DynamoDB API | `update_prometheus_kv()` using KV API (lines 410-431) |
| **Deployer script** | AWS CLI commands instead of Wrangler/Cloudflare API | `deployer/script.sh` (543 lines) |
| **Dockerfile stages** | Cross-compile `aarch64` instead of `wasm32` | Dockerfile lines 224-258 |

### Estimated new code

| File | Est. lines | Notes |
|------|-----------|-------|
| `src/resolver.rs` | ~300 | HTTP handler, routing, Host impl, suspend/resume, SQS send |
| `src/consumer.rs` | ~120 | SQS batch processing, DynamoDB metrics, log shipping |
| `src/materialization.rs` | ~150 | DynamoDB materialization store (read/write ops, key schema) |
| `src/common.rs` | ~120 | State init, AWS clients, CORS, env vars, store config |
| `Cargo.toml` | ~45 | Two binaries + AWS SDK deps |
| `Makefile` | ~15 | |
| `deployer/script.sh` | ~450 | Adapted from Cloudflare's 543-line script, includes DynamoDB table creation |
| `deployer/README.md` | ~120 | |
| Dockerfile additions | ~50 | 4 new stages |
| Root Makefile additions | ~5 | |
| **Total** | **~1375** | |

## Key Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Native vs WASM | Native Rust binary | Lambda supports custom runtimes. No WASM overhead. Full `std` support. |
| CPU architecture | ARM/Graviton (`aarch64`) | 20% cheaper than x86 on Lambda. Often faster for Rust. |
| State model | Compile-time embed | Matches Cloudflare pattern. No CDN dependency at runtime. |
| HTTP layer | Lambda Function URL | Simplest, cheapest. Direct HTTPS endpoint. Cloudflare Workers URL equivalent. |
| Event queue | SQS | Direct equivalent to Cloudflare Queues. Same batch config (100 msgs, 10s). |
| Metrics store | DynamoDB | Direct equivalent to Cloudflare KV. Same read-modify-write pattern. |
| Two binaries, one crate | `resolver` + `consumer` | Mirrors Cloudflare's single Worker with fetch + queue handlers. Shared deps. |
| Materializations | DynamoDB-backed (optional) | Equivalent to planned Cloudflare KV store. Suspend/resume protocol same as native Rust provider. Also supports remote store fallback. |
| API format | JSON | Same as Cloudflare. Uses `pbjson` serde impls. |

## What's NOT in scope (future work)

- **CloudFront integration**: Can be layered on top of Function URL later without code changes.
- **Lambda direct invoke (Service Bindings equivalent)**: Document in README as a usage pattern.
- **CloudWatch Embedded Metrics**: Future optimization — emit structured metrics directly to CloudWatch in addition to Prometheus endpoint.

## Verification

1. **Local build**: `cargo build --release` (native target) to verify compilation
2. **Docker build**: `docker build --target confidence-lambda-resolver.build .`
3. **Unit test**: Test routing logic, JSON parsing, state initialization
4. **Integration test**: Deploy to a test AWS account, hit Function URL:
   ```bash
   # Resolve flags
   curl -X POST https://<url>/v1/flags:resolve \
     -H "Content-Type: application/json" \
     -d '{"flags": ["flag-name"], "evaluation_context": {"targeting_key": "user-1"}}'

   # Check state
   curl https://<url>/v1/state:etag

   # Check metrics (if ENABLE_METRICS)
   curl -H "Authorization: ClientSecret <secret>" https://<url>/metrics
   ```
5. **SQS → Consumer flow**: Verify resolve requests produce SQS messages, consumer processes them, logs appear at `resolver.confidence.dev`
6. **DynamoDB metrics**: Verify `/metrics` returns valid Prometheus text after some resolves
7. **Materializations**: Test with a flag that has `MaterializationSpec`. Verify suspend/resume produces correct DynamoDB writes and subsequent reads return the stored variant
8. **Cold start benchmark**: Measure Lambda cold start time (target: <100ms)
9. **Deployer**: Run deployer container, verify ETag-based skip on no-change, verify deploy on state change
