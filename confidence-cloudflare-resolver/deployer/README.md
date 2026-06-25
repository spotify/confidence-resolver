# Confidence Cloudflare Resolver Deployer

Docker-based deployer that handles building and publishing the Confidence resolver Worker to your Cloudflare account. The resolver enables feature flag evaluation at Cloudflare's edge locations worldwide, powered by the [Confidence Resolver](https://github.com/spotify/confidence-resolver).

## Features

* **Edge evaluation**: Flag rules evaluate at Cloudflare's edge locations worldwide
* **Ultra-low latency**: Evaluation happens close to users, minimizing latency
* **Rust-based resolver**: High-performance flag evaluation powered by the Confidence Resolver
* **Deployer-driven sync**: Run the deployer to fetch the latest flag rules from Confidence and re-deploy the Worker

## Build

From the **root of the repository**, run:

```bash
docker build --target confidence-cloudflare-resolver.deployer -t <YOUR_IMAGE_NAME> .
```

A pre-built image is also available at `ghcr.io/spotify/confidence-cloudflare-deployer:latest`.

## Prerequisites

* Docker installed
* Cloudflare API token with the following permissions:
  * **Account > Workers Scripts > Edit**
  * **Account > Workers Queues > Edit** (needed for the first deploy)
  * **Account > Workers KV Storage > Edit** (only if using `ENABLE_METRICS` or `ENABLE_STICKY_ASSIGNMENTS_KV`)
* Confidence client secret (must be type **BACKEND**)

## Usage

Run the deployer with your credentials:

```bash
docker run -it \
    -e CLOUDFLARE_API_TOKEN='your-cloudflare-api-token' \
    -e CONFIDENCE_CLIENT_SECRET='your-confidence-client-secret' \
    ghcr.io/spotify/confidence-cloudflare-deployer:latest
```

The deployer automatically:

* **Detects Cloudflare account ID** from your API token
* **Creates the queue** (`flag-logs-queue`) if it doesn't exist
* **Fetches resolver state** from Confidence CDN
* **Skips deployment** if state hasn't changed (using ETags)

> **Note:** The deployer does not poll for changes. Each run fetches the current state from Confidence, deploys the Worker if the state has changed, and then exits. To keep the Worker up to date, run the deployer on a schedule (for example, via a cron job) or trigger it when flag rules or targeting changes are made in Confidence.

## Optional Variables

| Variable                             | Description                                                                                                                                       |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| `CLOUDFLARE_ACCOUNT_ID`              | Required only if the API token has access to multiple accounts                                                                                    |
| `CONFIDENCE_RESOLVER_STATE_URL`      | Custom resolver state URL (overrides default URL to Confidence CDN)                                                                               |
| `CONFIDENCE_RESOLVER_ALLOWED_ORIGIN` | Configure allowed origins for CORS                                                                                                                |
| `RESOLVE_TOKEN_ENCRYPTION_KEY`       | AES-128 key (base64 encoded) used to encrypt resolve tokens when `apply=false`. Not needed since the resolver defaults `apply` to `true`          |
| `FORCE_DEPLOY`                       | Force re-deploy regardless of state changes                                                                                                       |
| `NO_DEPLOY`                          | Build only, skip deployment                                                                                                                       |
| `WORKER_NAME_PREFIX`                 | Prefix for worker and queue names. Deploys as `<prefix>-confidence-cloudflare-resolver` with queue `<prefix>-flag-logs-queue` (auto-created)     |
| `WRANGLER_CONFIG_APPEND_FILE`        | Path to a file containing TOML to append to the generated `wrangler.toml`                                                                          |
| `WRANGLER_DEPLOY_TAG`                | Value passed to `wrangler deploy --tag`                                                                                                           |
| `WRANGLER_DEPLOY_MESSAGE`            | Value passed to `wrangler deploy --message`                                                                                                       |
| `WRANGLER_DEPLOY_ARGS`               | Additional newline-separated arguments passed to `wrangler deploy`                                                                                |
| `WRANGLER_DEPLOY_ARGS_FILE`          | Path to a file containing additional `wrangler deploy` arguments, one argument per line                                                           |
| `ENABLE_METRICS`                     | Set to create a KV namespace and enable the `/metrics` Prometheus endpoint. Requires a [KV store](https://developers.cloudflare.com/kv/platform/pricing/) |
| `ENABLE_STICKY_ASSIGNMENTS_KV`       | Set to enable sticky assignments backed by [Cloudflare KV](https://developers.cloudflare.com/kv/). Best for high-read, globally distributed workloads. Mutually exclusive with `ENABLE_STICKY_ASSIGNMENTS_DO` |
| `ENABLE_STICKY_ASSIGNMENTS_DO`       | Set to enable sticky assignments backed by [Durable Objects](https://developers.cloudflare.com/durable-objects/). Provides strong consistency per user. Mutually exclusive with `ENABLE_STICKY_ASSIGNMENTS_KV` |
| `MATERIALIZATION_TTL_SECONDS`        | TTL in seconds for KV-backed sticky assignments. Omit for no expiration |

### Extending Wrangler Configuration

Use `WRANGLER_CONFIG_APPEND_FILE` when your Cloudflare account needs configuration that is not managed by the deployer, such as observability destinations or tail consumers.

Example:

`wrangler-extra.toml`

```toml
[[tail_consumers]]
service = "my-tail-worker"

[observability.logs]
enabled = true
destinations = ["otel-gateway-logs"]
head_sampling_rate = 1.0
```

```bash
docker run -it \
    -v "$PWD/wrangler-extra.toml:/tmp/wrangler-extra.toml:ro" \
    -e CLOUDFLARE_API_TOKEN='your-cloudflare-api-token' \
    -e CONFIDENCE_CLIENT_SECRET='your-confidence-client-secret' \
    -e WRANGLER_CONFIG_APPEND_FILE='/tmp/wrangler-extra.toml' \
    -e WRANGLER_DEPLOY_TAG='production-2026-05-05' \
    -e WRANGLER_DEPLOY_MESSAGE='Deploy resolver state with tail worker logs' \
    ghcr.io/spotify/confidence-cloudflare-deployer:latest
```

The snippet is appended after the deployer has written its generated settings. To avoid top-level keys being parsed inside an existing table, the first non-comment line must be a TOML table header such as `[[tail_consumers]]` or `[observability.logs]`.

### Extending Wrangler Deploy

Use `WRANGLER_DEPLOY_TAG` and `WRANGLER_DEPLOY_MESSAGE` to label the deployed Worker version and deployment in Cloudflare.

```bash
docker run -it \
    -e CLOUDFLARE_API_TOKEN='your-cloudflare-api-token' \
    -e CONFIDENCE_CLIENT_SECRET='your-confidence-client-secret' \
    -e WRANGLER_DEPLOY_TAG='production-2026-05-05' \
    -e WRANGLER_DEPLOY_MESSAGE='Update embedded resolver state' \
    ghcr.io/spotify/confidence-cloudflare-deployer:latest
```

For less common Wrangler deploy flags, use `WRANGLER_DEPLOY_ARGS` or `WRANGLER_DEPLOY_ARGS_FILE` with one argument per line. Prefer `WRANGLER_DEPLOY_TAG` and `WRANGLER_DEPLOY_MESSAGE` for tags and messages so values may contain spaces safely.

## Service Binding vs HTTP Calls

When integrating with the Cloudflare resolver, you have two options:

**Service binding (recommended)**: Cloudflare's [service bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/) allow Workers to call other Workers directly within Cloudflare's network. This internal routing bypasses the public internet, resulting in ultra-low latency.

**HTTP calls**: Standard HTTP requests to the resolver endpoint. Use this approach when calling from external services or client applications.

### Example: Service binding with `@spotify-confidence/sdk`

1. Add a service binding to your `wrangler.json`:

```json
{
  "name": "my-worker",
  "main": "src/index.ts",
  "compatibility_date": "2025-02-04",
  "services": [
    {
      "binding": "ConfidenceBinding",
      "service": "confidence-cloudflare-resolver"
    }
  ]
}
```

2. Use the SDK with `fetchImplementation` and `waitUntil`:

```typescript
import { Confidence } from '@spotify-confidence/sdk';

interface Env {
  CONFIDENCE_CLIENT_SECRET: string;
  ConfidenceBinding: {
    fetch: (request: Request) => Promise<Response>;
  };
}

export default {
  async fetch(request, env, ctx): Promise<Response> {
    const confidence = Confidence.create({
      clientSecret: env.CONFIDENCE_CLIENT_SECRET,
      environment: 'backend',
      fetchImplementation: (req: Request) => env.ConfidenceBinding.fetch(req),
      timeout: 1000,
      waitUntil: (p) => ctx.waitUntil(p),
    });

    const flag = await confidence
      .withContext({ targeting_key: 'user-123' })
      .evaluateFlag('my-flag', {});

    return new Response(JSON.stringify({ flag }), {
      headers: { 'Content-Type': 'application/json' },
    });
  },
} satisfies ExportedHandler<Env>;
```

- **`fetchImplementation`** routes resolve requests through the service binding instead of the public internet.
- **`waitUntil`** keeps the Worker alive for background tasks (apply events, telemetry) after the response is sent. Without it, these fire-and-forget calls are silently dropped.
- **`environment: 'backend'`** is required for server-side usage.

For more details, see the [Confidence documentation](https://confidence.spotify.com/docs/sdks/edge/cloudflare#cloudflare-workers).

## Telemetry & Metrics

The resolver collects telemetry and exposes a Prometheus-compatible `/metrics` endpoint using the same metric names as all other Confidence providers (`confidence_resolve_latency_microseconds`, `confidence_resolves_total`).

### How latency is measured

Cloudflare Workers freeze `Date.now()` and `performance.now()` during synchronous CPU work (Spectre mitigation). The resolver uses `scheduler.wait(0)` — a zero-delay yield to the runtime — to unfreeze the clock after each resolve. This provides 1ms resolution with no measurable overhead.

### `/metrics` endpoint

Requires authentication:

```bash
curl -H "Authorization: ClientSecret <your-client-secret>" \
  https://<worker>.workers.dev/metrics
```

Returns Prometheus exposition format with:
- `confidence_resolve_latency_microseconds` — histogram (sum, count, cumulative `le` buckets)
- `confidence_resolves_total` — counter by resolve reason

Metrics are accumulated in a [KV namespace](https://developers.cloudflare.com/kv/platform/pricing/) (`CONFIDENCE_METRICS_KV`). Set `ENABLE_METRICS` to have the deployer create the KV namespace and bind it to the Worker. Without it, the `/metrics` endpoint returns empty and no KV writes occur.

### Backend telemetry

Resolve rates and latency are always sent to the Confidence backend via `WriteFlagLogsRequest`, regardless of the `ENABLE_METRICS` setting. The `/metrics` endpoint and KV store are only needed for direct Prometheus scraping — backend telemetry flows through the queue consumer independently.

## Sticky Assignments

Sticky assignments ensure users see the same experiment variant across requests. The deployer supports two storage backends — set one of the following environment variables to enable:

| Backend | Env var | Consistency | Read latency | Cost |
|---------|---------|-------------|--------------|------|
| **KV** | `ENABLE_STICKY_ASSIGNMENTS_KV` | Eventually consistent (~60s propagation) | <10ms (served from nearest edge) | Lower (per-read/write only) |
| **Durable Objects** | `ENABLE_STICKY_ASSIGNMENTS_DO` | Strongly consistent | <1ms if co-located, 50-200ms cross-region | Higher (per-request + duration + SQLite row reads/writes, but idle DOs hibernate at no cost) |

**KV is recommended** for most use cases — sticky assignments are read-heavy with write-once-per-user semantics, which plays to KV's strengths. Durable Objects hibernate when idle (no duration charges while sleeping), so cost scales with active processing time rather than idle time. Use Durable Objects if you need strong consistency guarantees (e.g., mutual exclusion between experiments).

The two backends are mutually exclusive. Without either variable set, sticky assignments are disabled and flags requiring them will return "flag not found".

## Limitations

* **Immediate apply**: The Cloudflare resolver forces `apply=true` on every resolve request, regardless of what the client SDK sends. This means:
  * Flag exposures are logged immediately at resolve time, before the flag value is rendered or shown to the user.
  * No resolve token is returned to the client, so the SDK's deferred apply mechanism is effectively disabled — apply calls from the SDK are accepted but silently discarded.
