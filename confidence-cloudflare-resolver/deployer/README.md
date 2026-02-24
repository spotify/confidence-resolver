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
* Confidence client secret (must be type **BACKEND**)

## Usage

On the first deploy, create the required Cloudflare Queue:

```bash
CLOUDFLARE_API_TOKEN='your-cloudflare-api-token' npx wrangler queues create flag-logs-queue
```

Then run the deployer with your credentials:

```bash
docker run -it \
    -e CLOUDFLARE_API_TOKEN='your-cloudflare-api-token' \
    -e CONFIDENCE_CLIENT_SECRET='your-confidence-client-secret' \
    ghcr.io/spotify/confidence-cloudflare-deployer:latest
```

The deployer automatically detects:

* **Cloudflare account ID** from your API token
* **Resolver state** from Confidence CDN
* **Existing deployment** to avoid unnecessary re-deploys

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

## Service Binding vs HTTP Calls

When integrating with the Cloudflare resolver, you have two options:

**Service binding (recommended)**: Cloudflare's [service bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/) allow Workers to call other Workers directly within Cloudflare's network. This internal routing bypasses the public internet, resulting in ultra-low latency.

**HTTP calls**: Standard HTTP requests to the resolver endpoint. Use this approach when calling from external services or client applications.

For more details on integration, including code examples using the [`@spotify-confidence/sdk`](https://github.com/spotify/confidence-sdk-js), see the [Confidence documentation](https://confidence.spotify.com/docs/sdks/edge/cloudflare#cloudflare-workers).

## Limitations

* **Sticky assignments**: Not currently supported with the Cloudflare resolver. Flags with sticky assignment rules will return "flag not found".
