# CloudFlare Resolver Worker Deployer

Docker container used to deploy the Confidence Rust resolver to CloudFlare.

## Build

From the **root of the repository**, run:

```bash
docker build --target confidence-cloudflare-resolver.deployer -t <YOUR_IMAGE_NAME> .
```

## Usage

A CloudFlare queue named `flag-logs-queue` must be enabled on your CloudFlare project. If this is your first setup, make sure to run this first:
```
 wrangler queues create flag-logs-queue
```

Docker run to deploy the resolver and your latest flags state:
```bash
docker run -it \
    -e CLOUDFLARE_API_TOKEN='<your-cloudflare-api-token>' \
    -e CONFIDENCE_CLIENT_SECRET='<your-confidence-client-secret>' \
    <YOUR_IMAGE_NAME>
```

You can get the `CONFIDENCE_CLIENT_SECRET` from the Confidence dashboard, under `API Clients`.

## Optional Variables

| Variable | Description |
|----------|-------------|
| `CLOUDFLARE_ACCOUNT_ID` | Required only if the API token has access to multiple accounts |
| `CONFIDENCE_RESOLVER_STATE_URL` | Custom resolver state URL (overrides default URL to Confidence CDN) |
| `CONFIDENCE_RESOLVER_ALLOWED_ORIGIN` | Configure allowed origins for CORS |
| `RESOLVE_TOKEN_ENCRYPTION_KEY` | AES-128 key (base64 encoded) used to encrypt resolve tokens when `apply=false`. Not needed since the resolver defaults `apply` to `true` |
| `FORCE_DEPLOY` | Force re-deploy regardless of state changes |
| `NO_DEPLOY` | Build only, skip deployment |

## Sticky Assignments

Sticky assignments are not currently supported with the CloudFlare resolver.
Flags with sticky assignment rules will return "flag not found".