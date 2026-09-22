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

A pre-built image is also available at `ghcr.io/spotify/confidence-cloudflare-deployer:latest` for both `linux/amd64` and `linux/arm64`.

## Prerequisites

* Docker installed
* Cloudflare API token with the following permissions:
  * **Account > Workers Scripts > Edit**
  * **Account > Workers Queues > Edit** (needed for the first deploy)
  * **Account > Workers KV Storage > Edit** (only if using `ENABLE_METRICS` or `ENABLE_STICKY_ASSIGNMENTS`)
  * **Account > Logs > Edit** (only if using `FLAG_LOG_SINK=logpush`; also requires the Workers Paid plan)
  * **Account > Logs > Edit** (only if using `FLAG_LOG_SINK=logpush`)

  The deployer probes each of these against the resolved account before it
  creates anything, and exits naming the missing scope. Note that a token
  valid for one account returns an indistinguishable authentication error for
  another, so a token scoped to the wrong account fails here too — check the
  account id in the error against the one you expect.

  A `wrangler login` OAuth session is **not** sufficient: wrangler cannot
  request a Logpush scope at all, so `FLAG_LOG_SINK=logpush` requires a real
  API token created in the dashboard.
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
| `RESOLVE_TOKEN_ENCRYPTION_KEY`       | AES-128 key (base64-encoded, 16 bytes). Used to encrypt resolve tokens for `apply=false`. Auto-generated on first deploy if not provided, and stored as a Cloudflare Worker secret |
| `FORCE_DEPLOY`                       | Force re-deploy regardless of state changes                                                                                                       |
| `NO_DEPLOY`                          | Build only, skip deployment                                                                                                                       |
| `WORKER_NAME_PREFIX`                 | Prefix for worker and queue names. Deploys as `<prefix>-confidence-cloudflare-resolver` with queue `<prefix>-flag-logs-queue` (auto-created)     |
| `WRANGLER_CONFIG_APPEND_FILE`        | Path to a file containing TOML to append to the generated `wrangler.toml`                                                                          |
| `WRANGLER_DEPLOY_TAG`                | Value passed to `wrangler deploy --tag`                                                                                                           |
| `WRANGLER_DEPLOY_MESSAGE`            | Value passed to `wrangler deploy --message`                                                                                                       |
| `WRANGLER_DEPLOY_ARGS`               | Additional newline-separated arguments passed to `wrangler deploy`                                                                                |
| `WRANGLER_DEPLOY_ARGS_FILE`          | Path to a file containing additional `wrangler deploy` arguments, one argument per line                                                           |
| `ENABLE_METRICS`                     | Set to create a KV namespace and enable the `/metrics` Prometheus endpoint. Requires a [KV store](https://developers.cloudflare.com/kv/platform/pricing/) |
| `ENABLE_STICKY_ASSIGNMENTS`          | Set to create a KV namespace and enable sticky assignments for experiments. Requires a [KV store](https://developers.cloudflare.com/kv/platform/pricing/) |
| `MATERIALIZATION_TTL_SECONDS`        | TTL in seconds for sticky assignment KV entries. Omit for no expiration |
| `FORCE_APPLY`                        | Defaults to `true`: every resolve is treated as `apply=true` and assignments are logged at resolve time. Set to `false` to respect the `apply` value sent by SDKs (deferred-apply flow via `flags:apply`) |
| `ENABLE_APPLY_DEDUP`                 | Defaults to `true`: apply-event deduplication is enabled — repeated identical assignments within a 120s window are logged once, both at resolve time and across queue consumer batches. Set to `false` to disable |
| `FLAG_LOGS_QUEUE_COUNT`              | Number of flag-log queues (default `1`, positive integer up to `9999`). Messages are randomly distributed across them; all use the same consumer Worker. |
| `FLAG_LOG_SINK`                      | `queue` (default) or `logpush`. Selects how flag logs leave the Worker — see [Flag-log sinks](#flag-log-sinks) |
| `FLAG_LOGS_INGEST_TOKEN`             | Optional when `FLAG_LOG_SINK=logpush`. Shared secret Logpush presents on every POST to the ingest route. Generated and stored as a worker secret when unset; supply it to pin the value across deploys |

### Flag-log sinks

`FLAG_LOG_SINK` selects how flag logs get from the Worker to Confidence.

Switching sinks is safe in both directions, and the deployer does the work to
make it so:

* **`queue` → `logpush`.** Queue bindings and the queue consumer are kept
  under either sink, so messages a previous version already published still
  get drained after the switch.
* **`logpush` → `queue`.** Logpush lags by about a minute, so a batch already
  in flight arrives at the ingest route after the switch. The route stays
  mounted under either sink and delivers what it receives, so those records
  land rather than being rejected.

#### `queue` (default)

```
resolve → flag-logs-queue → queue consumer → Confidence
```

Each log is published to a queue shard; the consumer aggregates up to 100
messages before delivery. A publish that fails is dropped — there is no
in-isolate retry, because anything held between requests is lost when the
isolate is evicted and Cloudflare provides no shutdown hook to flush it.

#### `logpush`

```
resolve → console.log → Logpush → /v1/flagLogs:ingest → Confidence
```

The log is compressed and written to `console.log` with a `FLAGLOG ` prefix,
and Cloudflare's Logpush POSTs batches of the Worker's own trace events back
to the Worker, which aggregates and delivers them.

Nothing is retained in isolate memory between requests, and the console write
happens inline during the response rather than in a post-response hook, so an
eviction cannot take a log with it. The queue sink publishes from
`waitUntil`, which Cloudflare does not guarantee to run.

Cost scales differently, which is the main reason to choose it. Queues bill per
message and charge three operations each, so cost tracks the number of log
records. Logpush bills per *request* — which you serve anyway — at $0.05 per
million with the first 10 million included, and batches thousands of records
into each push.

The deployer provisions it: it sets `logpush = true`, generates an ingest
token and stores it as a worker secret, passes `CONFIDENCE_ACCOUNT_ID`, and
creates a `workers_trace_events` Logpush job pointed at the Worker's own
`/v1/flagLogs:ingest` route.

**How batches get delivered.** Logpush POSTs a gzipped batch of trace events
to the ingest route; the handler pulls out the flag logs, deduplicates
applies across the whole batch, aggregates, and delivers. Each POST is an
ordinary Worker invocation, so the handler side scales without a configured
concurrency limit — unlike a queue consumer, which caps at 250.

**Throughput is bounded by Logpush, not by the Worker.** Measured, Logpush
pushes to an HTTP destination **serially**: one POST completes before the
next begins. At roughly 500 ms per 5 MB batch that is about 2 batches per
second, or **~17,000 records/sec for a single job**. Records per second is
therefore a function of batch size, which is why `max_upload_records` is set
high rather than low.

Beyond that ceiling the options are to shard the resolver across several
worker scripts, each with its own job — Logpush filters on `ScriptName` — or
to stop aggregating in Cloudflare and have the ingest side read the logs
directly. Two Logpush jobs on one script do not help: filters cannot
partition events, so both would push the same records.

**Deduplication.** Applies are deduplicated in the ingest handler across the
whole batch. This is the only place cross-isolate duplicates can be caught: a
batch carries records from many isolates, while the resolver's own dedup
window only ever sees one isolate's traffic.

**Where logs are sent.** Each log line carries its own destination, taken
from the resolver state when the line is written, and the first destination
in a batch is used for the whole batch. The handler therefore needs no
resolver state — only the account id, which arrives as a variable.

**Payload encoding.** Logs are sent as `base64(gzip(protobuf))`. gzip is what
matters: `AppliedFlag` entries repeat the targeting key and share
`flags/…/rules/…/variants/…` path prefixes, so measured against realistic
data it shrinks a log about 5.5x. base64 keeps the result escape-free so it
does not inflate again inside the trace event's JSON.

**Size handling.** Cloudflare truncates a trace event's `logs` and
`exceptions` fields once their combined length reaches 16,384 characters,
counting exceptions first. That limit is fixed, and splitting across several
`console.log` calls does not help because it applies per trace event rather
than per line. The Worker caps a console line at 12,000 characters and
delivers anything larger inline instead. Measured, a single-flag exposure log
encodes to ~350 characters and a 57-flag one to ~3,800, so the cap engages
only for resolves applying a few hundred flags at once.

On the way out, the handler measures each aggregate and splits it to stay
under the backend's **4 MiB** limit — measured by bisection, with `413`
above it and no retry recovering.

**Job scoping.** The job is filtered to `ScriptName = <worker>` and
`EventType = fetch`.

Trade-offs versus the queue:

- **Delivery latency is roughly a minute** and is not tunable. Logpush's
  upload settings influence batch size, not latency.
- **A failed delivery is dropped**, loudly, and the handler always answers
  200. Returning an error would make Logpush retry, and Logpush responds to
  sustained failure by *disabling the job* — losing one batch is better than
  silently stopping the pipeline until someone notices. Alert on
  `flag log ingest: DROPPED`.
- **Flag logs are billed twice while `[observability]` is enabled.** Every
  `FLAGLOG` console line is also ingested by Workers Logs, on top of being
  POSTed to the ingest route.

Set `-e FORCE_DEPLOY=1` when switching sinks so an unchanged resolver state does
not skip deployment.

### Scaling flag-log queues

For traffic exceeding one queue's throughput, pass `-e FLAG_LOGS_QUEUE_COUNT=2`
to the Docker command. This keeps `flag-logs-queue` and adds `flag-logs-queue-2`,
with the same optional `WORKER_NAME_PREFIX` applied to both. Further queues use
suffixes `-3`, `-4`, and so on. The events queue is unaffected.

Use `-e FORCE_DEPLOY=1` when changing the count so an unchanged resolver state
does not skip deployment. Keep the count in subsequent deployer runs. Reducing
the count does not delete old queues; drain their backlog before removing their
consumer bindings by deploying the lower count.

Two queues spread 7,000 messages/sec to approximately 3,500 each, below
Cloudflare's 5,000 messages/sec per-queue limit. Allow headroom for bursts and
verify downstream processing capacity. This distributes traffic; it does not
add retries or durable fallback for rejected publishes.

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

Sticky assignments ensure users see the same experiment variant across requests. Set `ENABLE_STICKY_ASSIGNMENTS` to have the deployer create a [KV namespace](https://developers.cloudflare.com/kv/) and bind it to the Worker.

Each assignment is stored as a separate KV entry keyed by `mat:{unit}:{materialization}:{rule}`. This avoids read-modify-write races and allows all keys to be read/written independently. Components containing `:` are percent-encoded.

Without `ENABLE_STICKY_ASSIGNMENTS`, sticky assignments are disabled and flags requiring them will return "flag not found".

## Deferred Apply (`apply=false`)

The resolver supports deferred apply: when a client sends `apply=false`, the resolve response includes an encrypted resolve token instead of immediately logging the exposure. The client later sends this token to the `/v1/flags:apply` endpoint to record the exposure at the time the flag value was actually shown to the user.

An AES-128 encryption key is required for this flow. The deployer manages the key automatically:

1. If `RESOLVE_TOKEN_ENCRYPTION_KEY` is provided, it is used and stored as a Cloudflare Worker secret.
2. If not provided, the deployer checks if a secret already exists on the worker from a previous deploy.
3. If no secret exists, a random key is generated and stored as a Cloudflare Worker secret.

The key persists across deploys via Cloudflare's secret storage. To rotate the key, set `RESOLVE_TOKEN_ENCRYPTION_KEY` to a new value — tokens encrypted with the old key will no longer be valid.

## Limitations

* **No runtime state updates** — Flag rules only update on redeployment.
