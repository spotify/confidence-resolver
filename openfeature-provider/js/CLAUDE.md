# JavaScript OpenFeature Provider

## Overview

npm package: `@spotify-confidence/openfeature-server-provider-local`

TypeScript OpenFeature provider using the Confidence resolver compiled to WASM. Supports multiple WASM loading strategies and React Server Components.

## Entry Points & Exports

The package has 5 build targets configured in `tsdown.config.ts`:

| Export Path | Entry File | Platform | WASM Loading |
|-------------|-----------|----------|-------------|
| `"."` (default) | `src/index.inlined.ts` | neutral | WASM inlined as data URL |
| `"./node"` | `src/index.node.ts` | node | `fs.readFile` |
| `"./fetch"` | `src/index.fetch.ts` | neutral | `fetch()` (Deno, Bun, browsers) |
| `"./react-server"` | `src/react/server.tsx` | neutral | React Server Component |
| `"./react-client"` | `src/react/client.tsx` | neutral | React Client Component |

Each entry point exports a `createConfidenceServerProvider` factory function.

The `./node` entry point extends options with `wasmPath?: string` and `./fetch` with `wasmUrl?: URL | string`.

## ProviderOptions

Defined in `src/ConfidenceServerProviderLocal.ts`:

```typescript
interface ProviderOptions {
  flagClientSecret: string;
  initializeTimeout?: number;
  stateUpdateInterval?: number;  // ms between state polls (default: 30000)
  flushInterval?: number;        // ms between log flushes (default: 15000)
  fetch?: typeof fetch;
  materializationStore?: MaterializationStore | 'CONFIDENCE_REMOTE_STORE';
}
```

## Build & Test

```bash
make build      # build WASM (if needed) + yarn install + proto:gen + tsdown build
make test       # format:check + typecheck + vitest (excludes *.e2e.test.ts)
make test-e2e   # run E2E tests (requires credentials)
```

## Gotchas

- **Proto location**: Local `proto/` contains only `test-only.proto`. The actual protos live in `../../openfeature-provider/proto/` and are referenced in the `proto:gen` script. Generated TypeScript goes to `src/proto/` (git-ignored).
- **E2E test exclusion**: E2E tests are excluded by the Makefile (`--exclude='**/*.e2e.test.ts'`), not by vitest.config.ts.
- **Debug logging**: Namespace `cnfd` — use `DEBUG=cnfd:*` to enable.
- **Publishing**: Uses OIDC (npm Trusted Publishers) — no npm tokens in Docker.
