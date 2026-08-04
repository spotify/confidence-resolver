# Thin Confidence client for edge resolution

**Status:** Implemented
**Name:** `ConfidenceClient`
**Scope:** `openfeature-provider/js` only (other languages may follow the same
shape later)

**Implementation:** `openfeature-provider/js/src/ConfidenceClient.ts` (client and
`evaluate`), `src/index.remote.ts` (entry point), `src/ConfidenceClient.test.ts`
(unit, mocked transport), `src/ConfidenceClient.e2e.test.ts` (against the live
`resolver.confidence.dev`).

## Background

A common deployment runs flag resolution on Cloudflare: an application Worker
calls a dedicated Confidence resolver Worker (same colo, sub-millisecond). That
resolver used to hardcode `apply = true` — every resolve immediately counted as
an exposure.

The goal was to split that: **resolve server-side, apply client-side** — an
exposure should only be recorded when a flag actually affects what a user sees.
The resolver side landed first (`498dd2a6`: the resolver respects the `apply`
flag in the request and returns a resolve token when `apply=false`). This
document covers the application-side SDK that sends the applies.

The existing online SDK is the wrong tool for this. It is built around a
long-lived, stateful client — caching, context mutation, apply-on-access —
which fights the per-request execution model of a Worker, and it has no good
story for carrying a resolve token from a server-side resolve to a client-side
apply.

## Design

A thin, stateless flag client with three operations — `resolve`, `evaluate`,
`apply` — over plain `fetch`. No lifecycle, no background work, no state:
constructing one is free, so it can be created per request or shared, it makes
no difference.

Because the transport is just a fetch-compatible function, the same client
works:

- in an application Worker, calling the resolver Worker via a **service binding**
- in a Worker or Node, calling the resolver (or `resolver.confidence.dev`) over HTTP

In the browser, `evaluate` works on forwarded bundles with no network and no
secrets; exposure (`apply`) is proxied through the application Worker so the
client secret never leaves it (example below).

## API

```ts
interface ConfidenceClientOptions {
  flagClientSecret: string;
  /** Resolver base URL. Ignored path-wise when `fetch` is a service binding
   *  that routes by binding rather than hostname. */
  url?: string; // default: 'https://resolver.confidence.dev'
  /** fetch-compatible transport. Pass a Cloudflare service binding here. */
  fetch?: typeof fetch; // default: globalThis.fetch
}

class ConfidenceClient {
  constructor(options: ConfidenceClientOptions);

  /** Resolve the named flags — or all flags, when the array is empty. apply
   *  defaults to true (resolve counts as exposure); pass apply: false to
   *  defer exposure to an explicit apply(). */
  resolve(
    flagNames: string[], // [] resolves all flags
    context: Context,
    options?: { apply?: boolean },
  ): Promise<FlagBundle>;

  /** Record exposure for flags from an earlier resolve(..., {apply: false}).
   *  Every name must be covered by the token. */
  apply(resolveToken: string, flagNames: string | string[]): Promise<void>;
}

/**
 * Evaluate a (dot-path) flag key against a resolved bundle, with a typed
 * default. Pure function, no I/O — works on a bundle that was JSON-forwarded
 * from the server, so the browser evaluates without another resolve. Never
 * throws — errors surface as the default value with an ERROR reason.
 */
function evaluate<T>(
  bundle: FlagBundle,
  flagKey: string, // 'my-flag' or 'my-flag.some.field'
  defaultValue: T,
): ResolutionDetails<T>;

/** Plain object, passed through to targeting verbatim. */
type Context = { targeting_key?: string; [key: string]: unknown };

interface FlagBundle {
  flags: Record<string, ResolutionDetails<FlagObject | null>>;
  resolveToken: string; // opaque, encrypted; safe to forward to the browser
  resolveId: string;
}

interface ResolutionDetails<T> {
  value: T;
  variant?: string;
  reason: 'MATCH' | 'NO_SEGMENT_MATCH' | 'ERROR' | /* ... */ string;
  /** True when an apply is meaningful for this flag — skip applies otherwise. */
  shouldApply: boolean;
  errorCode?: string;
  errorMessage?: string;
}
```

The types deliberately match the shapes the resolver API and OpenFeature
already use (`ResolutionDetails`, resolve reasons) — no new vocabulary, and a
later OpenFeature integration consumes the same objects.

## End to end: TanStack Start on Cloudflare

The flow: resolve with `apply: false` in a server function, forward the
bundle to the front end (it's plain JSON), `evaluate` it there, and report
exposure back through the application Worker. The resolve token round-trips
through the browser — it is encrypted by the resolver and opaque to the
client — while the apply itself is proxied through a server function
(`createServerFn` always executes server-side), so the resolver stays
reachable only via the service binding and the client secret never leaves
the Worker.

Because the client is stateless, it can live at module level in the server
function file:

```ts
// src/confidence.server.ts
import { createServerFn } from '@tanstack/react-start';
import { env } from 'cloudflare:workers';
import { ConfidenceClient, type Context } from '...';

const confidence = new ConfidenceClient({
  flagClientSecret: env.CONFIDENCE_CLIENT_SECRET,
  fetch: (input, init) => env.RESOLVER.fetch(input, init), // service binding
});

export const resolveFlags = createServerFn({ method: 'POST' })
  .validator((data: { flags: string[]; context: Context }) => data)
  .handler(({ data }) => confidence.resolve(data.flags, data.context, { apply: false }));

export const applyFlag = createServerFn({ method: 'POST' })
  .validator((data: { resolveToken: string; flagName: string }) => data)
  .handler(({ data }) => confidence.apply(data.resolveToken, data.flagName));
```

Bundles are plain values and `evaluate` is a pure function, so resolving
several bundles with *different contexts* (user-scoped, page-scoped, …) is
just multiple loader values — there is no assumption of one bundle per
component tree:

```ts
// src/routes/product.$id.tsx
export const Route = createFileRoute('/product/$id')({
  loader: async ({ params }) => {
    const [userFlags, pageFlags] = await Promise.all([
      resolveFlags({ data: { flags: ['checkout-redesign'], context: { targeting_key: userId } } }),
      resolveFlags({ data: {
        flags: ['promo-banner'],
        context: { targeting_key: sessionId, page: 'product', product_id: params.id },
      } }),
    ]);
    return { userFlags, pageFlags };
  },
  component: ProductPage,
});
```

A minimal exposure hook — dedupe repeat applies, skip flags where
`shouldApply` is false, fire-and-forget through the server function:

```tsx
function useExposure(bundle: FlagBundle) {
  const applied = useRef(new Set<string>());
  return useCallback(
    (flagName: string) => {
      if (!bundle.flags[flagName]?.shouldApply || applied.current.has(flagName)) return;
      applied.current.add(flagName);
      applyFlag({ data: { resolveToken: bundle.resolveToken, flagName } }).catch(() => {});
    },
    [bundle],
  );
}

function ProductPage() {
  const { pageFlags } = Route.useLoaderData();
  const expose = useExposure(pageFlags);
  const banner = evaluate(pageFlags, 'promo-banner', { show: false, text: '' });

  useEffect(() => expose('promo-banner'), [expose]); // exposure when actually rendered

  return banner.value.show ? <Banner text={banner.value.text} /> : null;
}
```

(Exact `createServerFn` chaining — e.g. `validator` vs newer names — tracks
the TanStack Start version in use; the shape above is illustrative.)

## Semantics

- **`apply` defaults to `true`** on resolve. The safe default: naive usage
  never silently loses exposure data. Deferred apply is the explicit opt-in.
- **`resolve` rejects** on transport/HTTP errors — the caller decides the
  fallback. **`evaluate` never throws** — it returns the default with an
  ERROR reason (standard flag-SDK behavior), so rendering code stays
  branch-free.
- **Stateless by construction.** No initialize, no close, no timers. All
  state (flag definitions, sticky assignments, log shipping) lives in the
  resolver Worker.
- **`shouldApply`** on each flag tells the front end whether an apply is
  warranted, so it doesn't send pointless applies for e.g. archived flags.
- **`apply` is scoped to its token.** A resolve token only permits applying the
  flags it was minted for; naming any other flag rejects the call in full.
- **`evaluate(bundle, key, default)` is pure and tiny** — the browser bundle
  for a front end that only evaluates and applies stays minimal. The
  implementation already exists in the SDK (`flag-bundle.ts`); this is a
  re-export with a better name.

## Future direction: token-only applies

The apply path doesn't truly need the client secret. The resolve token is
encrypted by the resolver and only permits applying flags whose assignments
it contains — possession of a valid token is already the credential, scoped
more tightly than the secret is. The secret's only remaining role in apply is
attributing the exposure event to a client credential.

The token already carries the account; if the resolver also stamped the
client identity into it, `apply(resolveToken, flagNames)` could drop the
secret entirely — an additive, backward-compatible token change. At that
point browser-direct applies (the resolver Worker already serves CORS) become
clean: no secret in the browser, no proxy hop needed. Not required for v1 —
the Worker proxy above works today — but it removes the last reason the proxy
is *mandatory* rather than a choice.

## Dependencies & sequencing

- [x] Resolver: respects `apply` from the request and returns the resolve token
      when `apply=false` (`498dd2a6`). The apply endpoint already worked — it
      only short-circuited because forced-apply resolves return an empty token.
- [x] This client. Built as an entry point of the existing package rather than a
      new one, so it reuses the generated protos and `flag-bundle.ts` while
      shipping neither WASM nor OpenFeature (~9 kB gzipped).
- [ ] A dedicated SDK id for the thin client in resolve telemetry, so its
      traffic is distinguishable from the WASM-backed local provider. It reports
      `SDK_ID_JS_LOCAL_SERVER_PROVIDER` today — see the `TODO` in
      `ConfidenceClient.ts`. Additive proto change.

## Settled questions

- **Naming and packaging** — kept `ConfidenceClient`; shipped as `./remote` on
  the existing package.
- **Context spelling** — passthrough. `targeting_key`, the wire format, not
  OpenFeature's `targetingKey`.

## Still open

- The dedicated SDK id, above.
- Token-only applies, above — would remove the need to proxy applies at all.
- An integration test against the resolver Worker under `wrangler dev`. Deferred:
  it belongs beside the Worker in `confidence-cloudflare-resolver`, and running
  it in CI means adding `wrangler`, `worker-build` and a pinned
  `wasm-bindgen-cli` to a Docker stage. The e2e test against
  `resolver.confidence.dev` covers the same wire contract meanwhile.
