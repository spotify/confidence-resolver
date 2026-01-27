# Confidence OpenFeature Local Provider for JavaScript

OpenFeature provider for the Spotify Confidence resolver (local mode, powered by WebAssembly). It periodically fetches resolver state, evaluates flags locally, and flushes evaluation logs to the Confidence backend.

## Features

- Local flag evaluation via WASM (no per-eval network calls)
- Automatic state refresh and batched flag log flushing
- Pluggable `fetch` with retries, timeouts and routing
- Optional logging using `debug`
- **[React integration](./README-REACT.md)** for Next.js with Server Components

## Requirements

- Node.js 18+ (built-in `fetch`) or provide a compatible `fetch`
- WebAssembly support (Node 18+/modern browsers)

---

## Installation

```bash
yarn add @spotify-confidence/openfeature-server-provider-local

# Optional: enable logs by installing the peer dependency
yarn add debug
```

Notes:

- `debug` is an optional peer. Install it if you want logs. Without it, logging is a no-op.
- Types and bundling are ESM-first; Node is supported, and a browser build is provided for modern bundlers.

---

## Getting Your Credentials

You'll need a **client secret** from Confidence to use this provider.

**📖 See the [Integration Guide: Getting Your Credentials](../INTEGRATION_GUIDE.md#getting-your-credentials)** for step-by-step instructions on:

- How to navigate the Confidence dashboard
- Creating a Backend integration
- Creating a test flag for verification
- Best practices for credential storage

---

## Quick start (Node)

```ts
import { OpenFeature } from '@openfeature/server-sdk';
import { createConfidenceServerProvider } from '@spotify-confidence/openfeature-server-provider-local';

const provider = createConfidenceServerProvider({
  flagClientSecret: process.env.CONFIDENCE_FLAG_CLIENT_SECRET!,
  // initializeTimeout?: number
  // stateUpdateInterval?: number
  // flushInterval?: number
  // fetch?: typeof fetch (Node <18 or custom transport)
});

// Wait for the provider to be ready (fetches initial resolver state)
await OpenFeature.setProviderAndWait(provider);

const client = OpenFeature.getClient();

// Create evaluation context with user attributes for targeting
const context = {
  targetingKey: 'user-123',
  country: 'US',
  plan: 'premium',
};

// Evaluate a boolean flag
const enabled = await client.getBooleanValue('test-flag.enabled', false, context);
console.log('Flag value:', enabled);

// Evaluate a nested value from an object flag using dot-path
// e.g. flag key "experiments" with payload { groupA: { ratio: 0.5 } }
const ratio = await client.getNumberValue('experiments.groupA.ratio', 0, context);

// On shutdown, flush any pending logs
await provider.onClose();
```

---

## Evaluation Context

The evaluation context contains information about the user/session being evaluated for targeting and A/B testing.

### TypeScript/JavaScript Examples

```typescript
// Simple attributes
const context = {
  targetingKey: 'user-123',
  country: 'US',
  plan: 'premium',
  age: 25,
};
```

---

## Error Handling

The provider uses a **default value fallback** pattern - when evaluation fails, it returns your specified default value instead of throwing an error.

**📖 See the [Integration Guide: Error Handling](../INTEGRATION_GUIDE.md#error-handling)** for:

- Common failure scenarios
- Error codes and meanings
- Production best practices
- Monitoring recommendations

### TypeScript/JavaScript Examples

```typescript
// The provider returns the default value on errors
const enabled = await client.getBooleanValue('my-flag.enabled', false, context);
// enabled will be 'false' if evaluation failed

// For detailed error information, use getBooleanDetails()
const details = await client.getBooleanDetails('my-flag.enabled', false, context);
if (details.errorCode) {
  console.error('Flag evaluation error:', details.errorMessage);
  console.log('Reason:', details.reason);
}
```

---

## Options

- `flagClientSecret` (string, required): The flag client secret used during evaluation and authentication.
- `initializeTimeout` (number, optional): Max ms to wait for initial state fetch. Defaults to 30_000.
- `stateUpdateInterval` (number, optional): Interval in ms between state polling updates. Defaults to 30_000.
- `flushInterval` (number, optional): Interval in ms for sending evaluation logs. Defaults to 10_000.
- `fetch` (optional): Custom `fetch` implementation. Required for Node < 18; for Node 18+ you can omit.

The provider periodically:

- Refreshes resolver state (configurable via `stateUpdateInterval`, default every 30s)
- Flushes flag evaluation logs to the backend (configurable via `flushInterval`, default every 10s)

---

## Exports and WASM Loading

The package provides multiple exports for different environments:

### Default export (recommended)

```ts
import { createConfidenceServerProvider } from '@spotify-confidence/openfeature-server-provider-local';
```

The WASM is **inlined as a data URL** — this is the most portable option and should work across virtually all environments. The tradeoff is a larger bundle (~700kB), but this isn't a problem for the intended server-side usage.

No configuration needed.

### `./node` — Traditional Node.js

```ts
import { createConfidenceServerProvider } from '@spotify-confidence/openfeature-server-provider-local/node';
```

Uses `fs.readFile()` to load WASM from the installed package. Works well in a regular Node.js environment with node_modules.

You can customize the WASM path if needed:

```ts
const provider = createConfidenceServerProvider({
  flagClientSecret: '...',
  wasmPath: '/custom/path/to/confidence_resolver.wasm',
});
```

### `./fetch` — Modern and standards compliant environments

```ts
import { createConfidenceServerProvider } from '@spotify-confidence/openfeature-server-provider-local/fetch';
```

Uses `fetch()` with `import.meta.url` to load WASM. Works in Deno, Bun, and browsers with bundlers that properly handle asset URLs (Vite, Rollup, etc.).

You can customize the WASM URL if needed:

```ts
const provider = createConfidenceServerProvider({
  flagClientSecret: '...',
  wasmUrl: '/assets/confidence_resolver.wasm',
});
```

### `./react-server` and `./react-client` — React/Next.js Integration

```ts
// Server Component
import { ConfidenceProvider, useFlag, useFlagDetails } from '@spotify-confidence/openfeature-server-provider-local/react-server';

// Client Component
import { useFlag, useFlagDetails } from '@spotify-confidence/openfeature-server-provider-local/react-client';
```

React hooks and components for Next.js App Router with Server Components. Flags are resolved on the server and provided to client components via React Context.

**See [README-REACT.md](./README-REACT.md) for full documentation.**

### A note on browser usage

While browsers are mentioned in this doc, this package is intended for server-side use only. Two concerns for browser usage:

- Size: The WASM+JS is currently ~270kb gzipped, too large for typical client bundles.
- Security: In a browser, all flag rules and variants are exposed to users.

That said, the package does work in browsers, and there may be specialized use cases where these tradeoffs are acceptable.

---

## Materialization Stores

Materialization stores provide persistent storage for sticky variant assignments and custom targeting segments. This enables two key use cases:

1. **Sticky Assignments**: Maintain consistent variant assignments across evaluations even when targeting attributes change. This enables pausing intake (stopping new users from entering an experiment) while keeping existing users in their assigned variants.

2. **Custom Targeting via Materialized Segments**: Precomputed sets of identifiers from datasets that should be targeted. Instead of evaluating complex targeting rules at runtime, materializations allow efficient lookup of whether a unit (user, session, etc.) is included in a target segment.

### Default Behavior

⚠️ Warning: If your flags rely on sticky assignments or materialized segments, the default SDK behaviour will prevent those rules from being applied and your evaluations will fall back to default values. For production workloads that need sticky behavior or segment lookups, implement and configure a real `MaterializationStore` (e.g., Redis, DynamoDB, or a key-value store) to avoid unexpected fallbacks and ensure consistent variant assignment.

### Remote Materialization Store

For quick setup without managing your own storage infrastructure, enable the built-in remote materialization store:

```ts
const provider = createConfidenceServerProvider({
  flagClientSecret: process.env.CONFIDENCE_FLAG_CLIENT_SECRET!,
  materializationStore: 'CONFIDENCE_REMOTE_STORE',
});
```

**When to use**:

- You need sticky assignments or materialized segments but don't want to manage storage infrastructure
- Quick prototyping or getting started
- Lower-volume applications where network latency is acceptable

**Trade-offs**:

- Additional network calls during flag resolution (adds latency)
- Lower performance compared to local storage implementations (Redis, DynamoDB, etc.)

### Custom Implementations

For improved latency and reduced network calls, implement the `MaterializationStore` interface to store materialization data in your infrastructure:

```ts
import { MaterializationStore } from '@spotify-confidence/openfeature-server-provider-local';

class MyRedisStore implements MaterializationStore {
  async readMaterializations(readOps: MaterializationStore.ReadOp[]): Promise<MaterializationStore.ReadResult[]> {
    // Load materialization data from Redis
  }

  async writeMaterializations(writeOps: MaterializationStore.WriteOp[]): Promise<void> {
    // Store materialization data to Redis
  }
}

const provider = createConfidenceServerProvider({
  flagClientSecret: process.env.CONFIDENCE_FLAG_CLIENT_SECRET!,
  materializationStore: new MyRedisStore(),
});
```

For read-only stores (e.g., pre-populated materialized segments without sticky assignment writes), omit the `writeMaterializations` method.

### When to Use Materialization Stores

Consider implementing a materialization store if:

- You need to support sticky variant assignments for experiments
- You use materialized segments for custom targeting
- You want to minimize network latency during flag resolution
- You have high-volume flag evaluations

If you don't use sticky assignments or materialized segments, the default behavior is sufficient.

---

## Logging (optional)

Logging uses the `debug` library if present; otherwise, all log calls are no-ops.

Namespaces:

- Core: `cnfd:*`
- Fetch/middleware: `cnfd:fetch:*` (e.g. retries, auth renewals, request summaries)

Log levels are hierarchical:

- `cnfd:debug` enables debug, info, warn, and error
- `cnfd:info` enables info, warn, and error
- `cnfd:warn` enables warn and error
- `cnfd:error` enables error only

Enable logs:

- Node:

```bash
DEBUG=cnfd:* node app.js
# or narrower
DEBUG=cnfd:info,cnfd:fetch:* node app.js
```

- Browser (in DevTools console):

```js
localStorage.debug = 'cnfd:*';
```

Install `debug` if you haven’t:

```bash
yarn add debug
```

---

## Testing

- You can inject a custom `fetch` via the `fetch` option to stub network behavior in tests.
- The provider batches logs; call `await provider.onClose()` in tests to flush them deterministically.

---

## License

See the root `LICENSE`.

## Formatting

Code is formatted using prettier, you can format all files by running

```sh
yarn format
```
