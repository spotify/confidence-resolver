# Plan: `@spotify-confidence/openfeature-cloudflare-provider`

## Context

The Cloudflare resolver (`confidence-cloudflare-resolver/`) runs the Confidence flag resolver at the edge as a Worker. Today, the only JS OpenFeature integration is `@spotify-confidence/openfeature-server-provider-local` which embeds WASM in-process, polls CDN for state, manages log flushing, materializations, etc. (~2,500 LOC). That's way too heavy for consumers that just want to call the CF resolver via a Cloudflare Service Binding.

We need a slim OpenFeature provider (~150 LOC) that talks to the CF resolver over a Service Binding — no WASM, no state polling, no log flushing, no client secret. The CF resolver handles all of that.

## Approach

Two PRs, each small and independently reviewable.

### PR 1: Extract shared code into `openfeature-provider/js-shared/`

Create a plain TypeScript source directory (no `package.json`, no npm publish). Both packages import via relative paths; `tsdown` bundles everything inline so consumers get self-contained packages.

**Files to create in `openfeature-provider/js-shared/`:**

| File | Extracted from | Contents |
|------|---------------|----------|
| `types.ts` | `js/src/types.ts` | `ResolutionReason`, `ErrorCode`, `ResolutionDetails<T>`, `FlagValue`, `FlagObject` |
| `flag-bundle.ts` | `js/src/flag-bundle.ts` | `FlagBundle` interface, `resolve()`, `evaluateAssignment()`, `error()`, `encodeToken()`, `decodeToken()` |
| `util.ts` | `js/src/util.ts` | `hasKey`, `isObject`, `bytesFromBase64`, `base64FromBytes` |

**What stays in `js/src/`:**
- `flag-bundle.ts` keeps its proto-specific `create()` and `convertReason()`, imports shared parts from `../../js-shared/flag-bundle`
- `types.ts` becomes a re-export from `../../js-shared/types`
- `util.ts` re-exports the moved functions, keeps all scheduler/timeout/abort utils locally

**Shared `flag-bundle.ts` needs a minimal logger type:**
```ts
export interface Logger {
  warn(msg: string, ...args: any[]): void;
}
```
The `resolve()` function takes `logger?: Logger` — this avoids pulling in the full `debug`-based logger.

**Logging in CF Workers:** CF Workers support `console.log/warn/error` natively — output goes to Cloudflare's log stream (`wrangler tail`). The CF provider should accept an optional `logger` in its options so consumers can pass a custom logger, defaulting to `console`. No `debug` package needed — it doesn't work in Workers anyway.

**Verification:** All existing JS provider tests must pass unchanged. The re-exports ensure no breaking changes for consumers importing from the existing package. Also validate that the JS provider's dist files remain identical (same exports, no new chunks, no missing files).

### PR 2: New CF provider package + CF resolver client_secret fallback

#### A. CF resolver change (`confidence-cloudflare-resolver/src/lib.rs`)

Add client_secret fallback to env var when request body omits it. Two call sites (resolve + apply):

```rust
// Before (line ~244):
let (reasons, resp) = match state.get_resolver::<H>(
    &resolver_request.client_secret, ...

// After:
let effective_secret = if resolver_request.client_secret.is_empty() {
    CONFIDENCE_CLIENT_SECRET.get().map(|s| s.as_str()).unwrap_or("")
} else {
    &resolver_request.client_secret
};
let (reasons, resp) = match state.get_resolver::<H>(
    effective_secret, ...
```

Same pattern for the apply handler (~line 334). Backwards-compatible: existing callers that send `client_secret` are unaffected.

#### B. Update root CLAUDE.md

Add the new provider to the repository overview and key components list. Document what it is (slim OpenFeature provider for CF resolver via Service Binding), what it is NOT (no WASM, no state management, no materializations), and when to use it (Cloudflare Workers calling the CF resolver) vs the local provider (in-process WASM evaluation).

#### C. New package: `openfeature-provider/cloudflare/`

```
openfeature-provider/cloudflare/
  CLAUDE.md
  CHANGELOG.md
  Makefile
  package.json          # @spotify-confidence/openfeature-cloudflare-provider
  tsconfig.json
  tsdown.config.ts
  vitest.config.ts
  .yarnrc.yml
  prettier.config.cjs
  .prettierignore
  .gitignore
  src/
    index.ts                          # factory + re-exports
    ConfidenceCloudflareProvider.ts    # Provider class (~100 LOC)
    ConfidenceCloudflareProvider.test.ts
    cf-flag-bundle.ts                 # JSON response -> FlagBundle
    cf-flag-bundle.test.ts
    version.ts                        # release-please managed
```

**Provider API:**
```ts
interface CloudflareProviderOptions {
  resolverBinding: { fetch(input: RequestInfo, init?: RequestInit): Promise<Response> };
  ctx: { waitUntil(promise: Promise<any>): void };
  logger?: { warn(msg: string, ...args: any[]): void };
}

function createConfidenceCloudflareProvider(options: CloudflareProviderOptions): Provider;
```

- `resolverBinding` — Cloudflare Service Binding (`env.CONFIDENCE_RESOLVER`)
- `ctx` — Cloudflare `ExecutionContext`. The provider wraps service binding fetches in `ctx.waitUntil()` internally so the CF resolver's background work (log shipping via queue) survives after the response is sent. Same pattern as `@spotify-confidence/sdk`'s `waitUntil` option.
- `logger` — optional, defaults to `console`. CF Workers support `console.*` natively (`wrangler tail`).

**Key implementation details:**

1. **Service Binding first** — accepts a `Fetcher` (the CF Service Binding interface). The URL in the `fetch` call is irrelevant for service bindings (e.g., `http://resolver/v1/flags:resolve`).

2. **Always `apply: true`** — hardcoded, no option to disable. The CF resolver handles apply + log shipping.

3. **No client secret** — not in options, not sent in request body. CF resolver falls back to its env var.

4. **Stateless** — `initialize()` is a no-op, status is always `READY`. No polling, no flushing, no WASM. Can be created per-request in Workers.

5. **Context conversion** — `targetingKey` mapped to `targeting_key` (same as existing JS provider's `convertEvaluationContext`).

6. **JSON field naming** — CF resolver uses pbjson which serializes as camelCase (`evaluationContext`, `resolvedFlags`, `shouldApply`). Requests should use camelCase too.

7. **`cf-flag-bundle.ts`** — CF-specific `create()` that handles string reasons (`"RESOLVE_REASON_MATCH"`) from the JSON response. Imports shared `FlagBundle` type and `resolve()`/`evaluateAssignment()` from `js-shared/`.

8. **Error handling** — HTTP errors and network failures return `defaultValue` with `ErrorCode.GENERAL`.

**Consumer usage:**

The `resolver` option accepts any object with a `fetch` method — this is the Cloudflare `Fetcher` interface that Service Bindings expose on `env`. The hostname in the URL is ignored by service bindings; Cloudflare routes directly by binding name.

The `ctx` option takes the Cloudflare `ExecutionContext`. The provider uses `ctx.waitUntil()` internally to keep the Worker alive for the CF resolver's background work (log shipping via Cloudflare Queue). This mirrors the pattern used in `@spotify-confidence/sdk`. The consumer doesn't need to think about `waitUntil` — the provider handles it.

```ts
import { createConfidenceCloudflareProvider } from '@spotify-confidence/openfeature-cloudflare-provider';
import { OpenFeature } from '@openfeature/server-sdk';

interface Env {
  CONFIDENCE_RESOLVER: { fetch(input: RequestInfo, init?: RequestInit): Promise<Response> };
}

export default {
  async fetch(request: Request, env: Env, ctx: ExecutionContext) {
    // env.CONFIDENCE_RESOLVER is the Service Binding — calls the CF resolver
    // Worker directly within Cloudflare's network (no public internet RTT)
    // ctx keeps the Worker alive for the CF resolver's background log shipping
    const provider = createConfidenceCloudflareProvider({
      resolverBinding: env.CONFIDENCE_RESOLVER,
      ctx,
    });
    OpenFeature.setProvider(provider);
    const client = OpenFeature.getClient();
    const enabled = await client.getBooleanValue('my-flag.enabled', false, { targetingKey: 'user-123' });
    return new Response(enabled ? 'on' : 'off');
  },
};
```

The consumer's `wrangler.toml` (or `wrangler.json`) needs a service binding:
```toml
[[services]]
binding = "CONFIDENCE_RESOLVER"
service = "confidence-cloudflare-resolver"
```

#### C. Build/release integration

**Root Makefile** — add to `test`, `build`, `clean` targets.

**Dockerfile** — new stages following the JS provider pattern:
- `openfeature-provider-cloudflare-base` — node:20-alpine, yarn install, copy source + `js-shared/`
- `openfeature-provider-cloudflare.test` — run tests
- `openfeature-provider-cloudflare.build` — tsdown build + no-split verification
- `openfeature-provider-cloudflare.pack` — yarn pack
- `openfeature-provider-cloudflare.artifact` — extract tarball

No WASM dependency, no proto generation — this stage is fast and self-contained.

**release-please-config.json:**
```json
"openfeature-provider/cloudflare": {
  "path": "openfeature-provider/cloudflare",
  "release-type": "rust",
  "changelog-path": "CHANGELOG.md",
  "extra-files": ["package.json", "src/version.ts"]
}
```

**.release-please-manifest.json:**
```json
"openfeature-provider/cloudflare": "0.0.0"
```

**GitHub Actions** — add publish job for the CF provider (same OIDC/Trusted Publishers pattern as JS provider). Requires npm Trusted Publishers config for the new package name.

#### D. Dependencies

**Runtime**: none (zero deps). **Peer**: `@openfeature/core ^1.0.0` (optional). **Dev**: `@openfeature/server-sdk`, `tsdown`, `typescript`, `vitest`, `rolldown`, `prettier`.

No `@bufbuild/protobuf`, no `debug`, no React/Next.js. The package is tiny.

## Future: Unified provider with pluggable resolver

The shared code extraction in PR 1 and the CF provider in PR 2 are stepping stones toward a single `ConfidenceProvider` with a pluggable `Resolver` interface:

```ts
interface Resolver {
  resolve(flags: string[], context: Record<string, any>): Promise<FlagBundle>;
  initialize?(): Promise<void>;
  close?(): Promise<void>;
}
```

Three resolver implementations, same provider:

| Resolver | Backend | Use case |
|----------|---------|----------|
| `ServiceBindingResolver` | CF Worker via service binding | Cloudflare Workers at the edge |
| `LocalResolver` | In-process WASM + CDN state polling | High-throughput servers (Node, Deno, Bun) |
| `RemoteResolver` | HTTP to `resolver.confidence.dev` | Simple setups, low-volume backends |

**What this PR already does toward that goal:**
- `js-shared/flag-bundle.ts` extracts the shared `FlagBundle` type and `resolve()`/`evaluateAssignment()` — this becomes the contract between provider and resolver
- The CF provider's `evaluate()` method is essentially the unified provider pattern already (FlagBundle in, ResolutionDetails out)

**What a follow-up would need to change:**
- **Refactor `ConfidenceServerProviderLocal`** — move state polling, log flushing, materialization orchestration, and WASM lifecycle out of the provider class and into a `LocalResolver` implementation. This is the bulk of the work (~400 LOC moves from provider to resolver).
- **Extract the provider shell** — the ~50 LOC of OpenFeature glue (`evaluate()`, `resolveBooleanEvaluation()`, etc.) becomes the shared `ConfidenceProvider` class.
- **Unify `FlagBundle` creation** — each resolver normalizes its response format (protobuf, JSON, etc.) to `FlagBundle` internally, so the provider never sees protocol-specific types.
- **Package consolidation** — either merge into one package with entry points (`/local`, `/cloudflare`, `/remote`) or keep a core + resolver packages. A single package with entry points is simpler but means renaming from `@spotify-confidence/openfeature-server-provider-local` (breaking change). Separate resolver packages avoid the rename but add maintenance surface.

## Reused code

| Code | Location | Used by |
|------|----------|---------|
| `FlagBundle`, `resolve()`, `evaluateAssignment()` | `js-shared/flag-bundle.ts` | Both providers |
| `ResolutionDetails`, `ErrorCode`, `FlagValue` | `js-shared/types.ts` | Both providers |
| `hasKey`, base64 helpers | `js-shared/util.ts` | Both providers |
| `convertEvaluationContext` pattern | `js/src/ConfidenceServerProviderLocal.ts:360` | Replicated (3 lines) |

## Verification

**PR 1:**
- `make -C openfeature-provider/js test` — all existing tests pass
- `make -C openfeature-provider/js build` — bundle builds, no splitting
- Imports from `js-shared/` are bundled inline (check `dist/` has no external refs to `js-shared`)

**PR 2:**
- `make -C openfeature-provider/cloudflare test` — unit tests for provider + cf-flag-bundle
- `make -C openfeature-provider/cloudflare build` — clean build, no splitting
- `make -C confidence-cloudflare-resolver lint` — Rust changes pass clippy
- `docker build --target openfeature-provider-cloudflare.test .` — Docker build works
- Root `make test` and `make build` pass with new component included
