# Guide on migration from "online" provider to Local provider

This should guide you from moving from `@spotify-confidence/openfeature-server-provider` ([repo](https://github.com/spotify/confidence-sdk-js/blob/main/packages/openfeature-server-provider)) to `@spotify-confidence/openfeature-server-provider-local` ([repo](https://github.com/spotify/confidence-resolver/tree/main/openfeature-provider/js)).

## What's changing?

This migration moves from an **"online" evaluation model** (network call per flag) to a **"local" evaluation model** (WebAssembly-based evaluation with background state sync). This provides:

- **Near-zero latency** for flag evaluations (vs 50-100ms per call)
- **Higher throughput** for high-traffic applications
- **Better resilience** (continues working with cached state if temporarily disconnected)

The provider now uses client secret header authentication, simplifying the setup by requiring only the `flagClientSecret`.

## Dependencies

Simply switch out your version of `@spotify-confidence/openfeature-server-provider` to the latest version of `@spotify-confidence/openfeature-server-provider-local`.

The dependencies to Openfeature (`@openfeature/server-sdk` & `@openfeature/core`) should remain intact.

## Constructing

Where you previously used either type of approaches to construct your Confidence open feature provider:

```ts
const provider = createConfidenceServerProvider({
  clientSecret: 'your-client-secret',
  fetchImplementation: fetch,
  timeout: 1000,
});

// or
const confidenceInstance: Confidence; // created separately
const provider = createConfidenceServerProvider(confidenceInstance);
```

### The new constructor:

```ts
import { createConfidenceServerProvider } from '@spotify-confidence/openfeature-server-provider-local';

const provider = createConfidenceServerProvider({
  flagClientSecret: 'your-client-secret', // this is the same client secret as before
  encryptionKey: process.env.CONFIDENCE_CLIENT_ENCRYPTION_KEY!,
  // initializeTimeout?: number
  // flushInterval?: number
  // fetch?: typeof fetch (Node <18 or custom transport)
});
```

## Breaking Changes

### Parameter rename: `clientSecret` → `flagClientSecret`

The client id parameter has been renamed from `clientSecret` to `flagClientSecret`. The value can remain the same, just update the parameter name:

```ts
// Old
const provider = createConfidenceServerProvider({
  clientSecret: 'your-client-secret',
});

// New
const provider = createConfidenceServerProvider({
  flagClientSecret: 'your-client-secret',
  encryptionKey: process.env.CONFIDENCE_CLIENT_ENCRYPTION_KEY!,
});
```

### What happened to the `timeout` parameter?

The old provider used `timeout` to control network request timeouts for each flag evaluation. When exceeded, default values were returned.

The new provider works differently:

- Flag evaluations happen **locally in WebAssembly** (no network calls during evaluation)
- The optional `initializeTimeout` parameter (default: 30 seconds) controls how long to wait for the initial resolver state fetch
- You typically don't need to configure timeouts anymore

**Migration**: Remove the `timeout` parameter. If you need to control initialization wait time, use `initializeTimeout` instead.

## Usage

Since this is just another Provider meant to be used with the OpenFeature SDK, the integration when accessing flag values remain the same.

## Testing and verifying

As with any good software development practice we suggest you test this locally or in a test environment before launching in production.

To enable debug logging during testing, install the `debug` package and set the `DEBUG` environment variable:

```bash
yarn add debug
DEBUG=cnfd:* node your-app.js
```

This will show detailed logs about resolver state fetches, retries, and flag evaluations.

We suggest that you create a new flag in Confidence and resolve that flag using the new OpenFeature provider. To verify correctness you should:

#### 1 Verify the expected result in the flag evaluation in your application

#### 2 Verify that flag resolves are visible

<img src="../../img/resolve-graph.png" alt="resolve graph" width="300" />

#### 3 Verify applies are registered correctly - "last apply just now"

<img src="../../img/applies-registered.png" alt="applies registered" width="300" />

#### 4 Verify that flag rules are registering "resolved X times" updates

<img src="../../img/rule-resolved.png" alt="rule resolved" width="300" />
