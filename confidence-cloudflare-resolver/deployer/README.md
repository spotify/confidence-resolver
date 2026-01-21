# CloudFlare Resolver Worker Deployer

Docker container used to deploy the Confidence Rust resolver to CloudFlare.

# Build the image

From the **root of the repository**, run:

```
docker build -f confidence-cloudflare-resolver/deployer/Dockerfile -t <YOUR_IMAGE_NAME> .
```

# Usage

```
docker run -it \
	-e CLOUDFLARE_ACCOUNT_ID='<>' \
	-e CLOUDFLARE_API_TOKEN='<>' \
	-e CONFIDENCE_ACCOUNT_ID='<>' \
	-e CONFIDENCE_CLIENT_ID='<>' \
	-e CONFIDENCE_CLIENT_SECRET='<>' \
	-e RESOLVE_TOKEN_ENCRYPTION_KEY='<>' \
	-e CONFIDENCE_RESOLVER_STATE_ETAG_URL=‘<>/v1/state:etag' \
	image-name
```

The RESOLVE_TOKEN_ENCRYPTION_KEY key has to be a valid AES-128 (16 bytes) key, base64 encoded.
This key is used internally in the resolver, and shouldn't be changed once deployed in production.

The CONFIDENCE_RESOLVER_STATE_ETAG_URL needs to point to the resolver you deployed / are about to deploy. 
The `.../v1/state:etag` is the path used to retrieve the etag if available, ignored otherwise.
The etag value is used to avoid re-deploy the worker if the state hasn't changed since the last deploy.

Additional optional variables:
- CONFIDENCE_RESOLVER_STATE_URL: Point to a custom resolver state protobuf file;
- CONFIDENCE_RESOLVER_ALLOWED_ORIGIN: Configure allowed origins in the wrangler used to deploy the resolver;
- FORCE_DEPLOY: Re-deploy the resolver worker, regardless if the state is detected as changed or not.

# Sticky Assignments
Sticky assignments are currently not supported with the CloudFlare resolver.
If a flag with sticky assignment rules is encountered, it is ignored by the resolver and the evaluation will return a "flag not found".
