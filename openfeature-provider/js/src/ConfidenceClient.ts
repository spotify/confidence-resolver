import { ApplyFlagsRequest, ResolveFlagsRequest, ResolveFlagsResponse } from './proto/confidence/flags/resolver/v1/api';
import { SdkId } from './proto/confidence/flags/resolver/v1/types';
import FlagBundleType, * as FlagBundle from './flag-bundle';
import type { JsonValue, ResolutionDetails } from './types';
import { logger } from './logger';
import { VERSION } from './version';

const DEFAULT_URL = 'https://resolver.confidence.dev';
const FLAG_PREFIX = 'flags/';

// TODO: a dedicated SDK id for the thin client would make its resolve traffic
// distinguishable from the WASM-backed local provider. Additive proto change.
const SDK = { id: SdkId.SDK_ID_JS_LOCAL_SERVER_PROVIDER, version: VERSION };

/** Evaluation context, passed through to targeting verbatim. */
export type Context = { targeting_key?: string; [key: string]: unknown };

export type { default as FlagBundle } from './flag-bundle';

export interface ConfidenceClientOptions {
  flagClientSecret: string;
  /**
   * Resolver base URL. Also used for the request path when `fetch` is a
   * Cloudflare service binding (bindings route by binding, not by hostname).
   */
  url?: string;
  /** fetch-compatible transport. Pass a Cloudflare service binding here. */
  fetch?: typeof fetch;
}

/**
 * A thin, stateless flag client for use against a remote resolver — a
 * Confidence resolver Worker reached via service binding, or
 * `resolver.confidence.dev` over HTTP.
 *
 * There is no lifecycle, no background work and no cached state: constructing
 * one is free, so it can be created per request or shared at module level —
 * it makes no difference.
 */
export class ConfidenceClient {
  private readonly clientSecret: string;
  private readonly baseUrl: string;
  private readonly fetchImpl: typeof fetch;

  constructor(options: ConfidenceClientOptions) {
    this.clientSecret = options.flagClientSecret;
    // Trailing slashes would produce '//v1/flags:resolve'.
    this.baseUrl = (options.url ?? DEFAULT_URL).replace(/\/+$/, '');
    this.fetchImpl = options.fetch ?? globalThis.fetch;
  }

  /**
   * Resolve the named flags — or all flags available to the client, when the
   * array is empty.
   *
   * `apply` defaults to true, so a resolve counts as an exposure. Pass
   * `{ apply: false }` to defer exposure to an explicit {@link apply} call;
   * the returned bundle then carries a resolve token to apply against.
   *
   * Rejects on transport and HTTP errors — the caller decides the fallback.
   */
  async resolve(flagNames: string[], context: Context, options?: { apply?: boolean }): Promise<FlagBundleType> {
    const request = ResolveFlagsRequest.create({
      flags: flagNames.map(name => FLAG_PREFIX + name),
      evaluationContext: context,
      apply: options?.apply ?? true,
      clientSecret: this.clientSecret,
      sdk: SDK,
    });
    const response = await this.post('/v1/flags:resolve', ResolveFlagsRequest.toJSON(request));
    return FlagBundle.create(ResolveFlagsResponse.fromJSON(await response.json()));
  }

  /**
   * Record exposure for flags from an earlier `resolve(..., { apply: false })`.
   *
   * Skip flags whose `shouldApply` is false — an apply is meaningless for them.
   * Rejects on transport and HTTP errors.
   *
   * A token only permits applying the flags it was minted for; naming any
   * other flag rejects the call in full.
   */
  async apply(resolveToken: string, flagNames: string | string[]): Promise<void> {
    const names = typeof flagNames === 'string' ? [flagNames] : flagNames;
    // A resolve with apply=true returns no token, and there is nothing to
    // apply for an empty flag list — save the round trip either way.
    if (!resolveToken || names.length === 0) return;

    const now = new Date();
    const request = ApplyFlagsRequest.create({
      flags: names.map(name => ({ flag: FLAG_PREFIX + name, applyTime: now })),
      clientSecret: this.clientSecret,
      resolveToken: FlagBundle.decodeToken(resolveToken),
      sendTime: now,
      sdk: SDK,
    });
    await this.post('/v1/flags:apply', ApplyFlagsRequest.toJSON(request));
  }

  private async post(path: string, body: unknown): Promise<Response> {
    const response = await this.fetchImpl(`${this.baseUrl}${path}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
    if (!response.ok) {
      // The resolver returns diagnostics as the body (e.g. "client secret not
      // found: requested=..., available=[...]") — worth surfacing.
      const detail = await response.text().catch(() => '');
      throw new Error(
        `Confidence ${path} failed: ${response.status} ${response.statusText}${detail ? ` - ${detail}` : ''}`,
      );
    }
    return response;
  }
}

/**
 * Evaluate a flag key against a resolved bundle, with a typed default.
 *
 * A pure function with no I/O — it works on a bundle that was JSON-forwarded
 * from the server, so a browser can evaluate without resolving again. Never
 * throws: errors surface as the default value with an `ERROR` reason.
 *
 * @param flagKey - `'my-flag'` or a dot path into the value, `'my-flag.some.field'`
 */
export function evaluate<T extends JsonValue>(
  bundle: FlagBundleType,
  flagKey: string,
  defaultValue: T,
): ResolutionDetails<T> {
  return FlagBundle.resolve(bundle, flagKey, defaultValue, logger);
}
