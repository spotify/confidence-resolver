import type {
  ErrorCode,
  EvaluationContext,
  JsonValue,
  Provider,
  ProviderMetadata,
  ProviderStatus,
  ResolutionDetails,
  ResolutionReason,
} from '@openfeature/server-sdk';
import { ResolveFlagsRequest, ResolveFlagsResponse } from './proto/confidence/flags/resolver/v1/api';
import { ResolveWithStickyRequest } from './proto/confidence/wasm/wasm_api';
import { SdkId, ResolveReason } from './proto/confidence/flags/resolver/v1/types';
import { VERSION } from './version';
import { Fetch, withLogging, withResponse, withRetry, withRouter, withStallTimeout, withTimeout } from './fetch';
import { scheduleWithFixedInterval, timeoutSignal, TimeUnit } from './util';
import { LocalResolver } from './LocalResolver';
import { sha256Hex } from './hash';
import { getLogger } from './logger';

const logger = getLogger('provider');

export const DEFAULT_STATE_INTERVAL = 30_000;
export const DEFAULT_FLUSH_INTERVAL = 10_000;
export interface ProviderOptions {
  flagClientSecret: string;
  initializeTimeout?: number;
  flushInterval?: number;
  fetch?: typeof fetch;
}

/**
 * OpenFeature Provider for Confidence Server SDK (Local Mode)
 * @public
 */
export class ConfidenceServerProviderLocal implements Provider {
  /** Static data about the provider */
  readonly metadata: ProviderMetadata = {
    name: 'ConfidenceServerProviderLocal',
  };
  /** Current status of the provider. Can be READY, NOT_READY, ERROR, STALE and FATAL. */
  status = 'NOT_READY' as ProviderStatus;

  private readonly main = new AbortController();
  private readonly fetch: Fetch;
  private readonly flushInterval: number;
  private stateEtag: string | null = null;

  // TODO Maybe pass in a resolver factory, so that we can initialize it in initialize and transition to fatal if not.
  constructor(private resolver: LocalResolver, private options: ProviderOptions) {
    this.flushInterval = options.flushInterval ?? DEFAULT_FLUSH_INTERVAL;
    this.fetch = Fetch.create(
      [
        withRouter({
          'https://confidence-resolver-state-cdn.spotifycdn.com/*': [
            withRetry({
              maxAttempts: Infinity,
              baseInterval: 500,
              maxInterval: DEFAULT_STATE_INTERVAL,
            }),
            withStallTimeout(500),
          ],
          'https://resolver.confidence.dev/*': [
            withRouter({
              '*/v1/flags:resolve': [
                withRetry({
                  maxAttempts: 3,
                  baseInterval: 100,
                }),
                withTimeout(3 * TimeUnit.SECOND),
              ],
              '*/v1/clientFlagLogs:write': [
                withRetry({
                  maxAttempts: 3,
                  baseInterval: 500,
                }),
                withTimeout(5 * TimeUnit.SECOND),
              ],
            }),
          ],
          '*': [
            withResponse(url => {
              throw new Error(`Unknown route ${url}`);
            }),
          ],
        }),
        withLogging(),
      ],
      options.fetch ?? fetch,
    );
  }

  async initialize(context?: EvaluationContext): Promise<void> {
    // TODO validate options and switch to fatal.
    const signal = this.main.signal;
    const initialUpdateSignal = AbortSignal.any([
      signal,
      timeoutSignal(this.options.initializeTimeout ?? DEFAULT_STATE_INTERVAL),
    ]);
    try {
      // TODO set schedulers irrespective of failure
      // TODO if 403 here,
      await this.updateState(initialUpdateSignal);
      scheduleWithFixedInterval(signal => this.flush(signal), this.flushInterval, { maxConcurrent: 3, signal });
      // TODO Better with fixed delay so we don't do a double fetch when we're behind. Alt, skip if in progress
      scheduleWithFixedInterval(signal => this.updateState(signal), DEFAULT_STATE_INTERVAL, { signal });
      this.status = 'READY' as ProviderStatus;
    } catch (e: unknown) {
      this.status = 'ERROR' as ProviderStatus;
      // TODO should we swallow this?
      throw e;
    }
  }

  async onClose(): Promise<void> {
    await this.flush(timeoutSignal(3000));
    this.main.abort();
  }

  // TODO test unknown flagClientSecret
  async evaluate<T>(flagKey: string, defaultValue: T, context: EvaluationContext): Promise<ResolutionDetails<T>> {
    const [flagName, ...path] = flagKey.split('.');

    // Build resolve request
    // Always use sticky resolve request with remote fallback
    const stickyRequest: ResolveWithStickyRequest = {
      resolveRequest: {
        flags: [`flags/${flagName}`],
        evaluationContext: ConfidenceServerProviderLocal.convertEvaluationContext(context),
        apply: true,
        clientSecret: this.options.flagClientSecret,
        sdk: {
          id: SdkId.SDK_ID_JS_LOCAL_SERVER_PROVIDER,
          version: VERSION,
        },
      },
      materializations: [],
      failFastOnSticky: true, // Always fail fast - use remote resolver for sticky assignments
      notProcessSticky: false,
    };

    const response = await this.resolveWithStickyInternal(stickyRequest);
    const result = this.extractValue(response.resolvedFlags[0], flagName, path, defaultValue);

    if (result.errorCode) {
      logger.warn(`Flag evaluation for '${flagKey}' returned error code: ${result.errorCode}`);
    }

    return result;
  }

  /**
   * Internal recursive method for resolving with sticky assignments.
   *
   * @private
   */
  private async resolveWithStickyInternal(request: ResolveWithStickyRequest): Promise<ResolveFlagsResponse> {
    const response = this.resolver.resolveWithSticky(request);

    if (response.success && response.success.response) {
      this.flushAssigned();
      const { response: flagsResponse } = response.success;
      return flagsResponse;
    }

    // Handle missing materializations by falling back to remote resolver
    if (response.readOpsRequest) {
      return await this.remoteResolve(request.resolveRequest!);
    }

    throw new Error('Invalid response: resolve result not set');
  }

  private async remoteResolve(request: ResolveFlagsRequest): Promise<ResolveFlagsResponse> {
    const url = 'https://resolver.confidence.dev/v1/flags:resolve';

    const resp = await this.fetch(url, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
      },
      body: JSON.stringify(ResolveFlagsRequest.toJSON(request)),
    });

    if (!resp.ok) {
      throw new Error(`Remote resolve failed: ${resp.status} ${resp.statusText}`);
    }

    const json = await resp.json();
    return ResolveFlagsResponse.fromJSON(json);
  }

  /**
   * Extract and validate the value from a resolved flag.
   */
  private extractValue<T>(flag: any, flagName: string, path: string[], defaultValue: T): ResolutionDetails<T> {
    if (!flag) {
      return {
        value: defaultValue,
        reason: 'ERROR',
        errorCode: 'FLAG_NOT_FOUND' as ErrorCode,
      };
    }

    if (flag.reason !== ResolveReason.RESOLVE_REASON_MATCH) {
      return {
        value: defaultValue,
        reason: ConfidenceServerProviderLocal.convertReason(flag.reason),
      };
    }

    let value: unknown = flag.value;
    for (const step of path) {
      if (typeof value !== 'object' || value === null || !hasKey(value, step)) {
        return {
          value: defaultValue,
          reason: 'ERROR',
          errorCode: 'TYPE_MISMATCH' as ErrorCode,
        };
      }
      value = value[step];
    }

    if (!isAssignableTo(value, defaultValue)) {
      return {
        value: defaultValue,
        reason: 'ERROR',
        errorCode: 'TYPE_MISMATCH' as ErrorCode,
      };
    }

    return {
      value,
      reason: 'MATCH',
      variant: flag.variant,
    };
  }

  async updateState(signal?: AbortSignal): Promise<void> {
    // Build CDN URL using SHA256 hash of client secret
    const hashHex = await sha256Hex(this.options.flagClientSecret);
    const cdnUrl = `https://confidence-resolver-state-cdn.spotifycdn.com/${hashHex}`;

    const headers = new Headers();
    if (this.stateEtag) {
      headers.set('If-None-Match', this.stateEtag);
    }
    const resp = await this.fetch(cdnUrl, { headers, signal });
    if (resp.status === 304) {
      // not changed
      return;
    }
    if (!resp.ok) {
      throw new Error(`Failed to fetch state: ${resp.status} ${resp.statusText}`);
    }
    this.stateEtag = resp.headers.get('etag');

    // Parse SetResolverStateRequest from response
    const bytes = new Uint8Array(await resp.arrayBuffer());
    const { SetResolverStateRequest } = await import('./proto/confidence/wasm/messages');

    this.resolver.setResolverState(SetResolverStateRequest.decode(bytes));
  }

  // TODO should this return success/failure, or even throw?
  async flush(signal?: AbortSignal): Promise<void> {
    const writeFlagLogRequest = this.resolver.flushLogs();
    if (writeFlagLogRequest.length > 0) {
      await this.sendFlagLogs(writeFlagLogRequest, signal);
    }
  }

  private async flushAssigned(): Promise<void> {
    const writeFlagLogRequest = this.resolver.flushAssigned();
    if (writeFlagLogRequest.length > 0) {
      await this.sendFlagLogs(writeFlagLogRequest);
    }
  }

  private async sendFlagLogs(encodedWriteFlagLogRequest: Uint8Array, signal = this.main.signal): Promise<void> {
    try {
      const response = await this.fetch('https://resolver.confidence.dev/v1/clientFlagLogs:write', {
        method: 'post',
        signal,
        headers: {
          'Content-Type': 'application/x-protobuf',
          Authorization: `ClientSecret ${this.options.flagClientSecret}`,
        },
        body: encodedWriteFlagLogRequest as Uint8Array<ArrayBuffer>,
      });
      if (!response.ok) {
        logger.error(`Failed to write flag logs: ${response.status} ${response.statusText} - ${await response.text()}`);
      }
    } catch (err) {
      // Network error (DNS/connect/TLS) - already retried by middleware, log and rethrow
      logger.warn('Failed to send flag logs', err);
      throw err;
    }
  }

  private static convertReason(reason: ResolveReason): ResolutionReason {
    switch (reason) {
      case ResolveReason.RESOLVE_REASON_ERROR:
        return 'ERROR';
      case ResolveReason.RESOLVE_REASON_FLAG_ARCHIVED:
        return 'FLAG_ARCHIVED';
      case ResolveReason.RESOLVE_REASON_MATCH:
        return 'MATCH';
      case ResolveReason.RESOLVE_REASON_NO_SEGMENT_MATCH:
        return 'NO_SEGMENT_MATCH';
      case ResolveReason.RESOLVE_REASON_TARGETING_KEY_ERROR:
        return 'TARGETING_KEY_ERROR';
      case ResolveReason.RESOLVE_REASON_NO_TREATMENT_MATCH:
        return 'NO_TREATMENT_MATCH';
      default:
        return 'UNSPECIFIED';
    }
  }

  private static convertEvaluationContext({ targetingKey: targeting_key, ...rest }: EvaluationContext): {
    [key: string]: any;
  } {
    return {
      targeting_key,
      ...rest,
    };
  }

  /** Resolves with an evaluation of a Boolean flag */
  resolveBooleanEvaluation(
    flagKey: string,
    defaultValue: boolean,
    context: EvaluationContext,
  ): Promise<ResolutionDetails<boolean>> {
    return Promise.resolve(this.evaluate(flagKey, defaultValue, context));
  }
  /** Resolves with an evaluation of a Numbers flag */
  resolveNumberEvaluation(
    flagKey: string,
    defaultValue: number,
    context: EvaluationContext,
  ): Promise<ResolutionDetails<number>> {
    return Promise.resolve(this.evaluate(flagKey, defaultValue, context));
  }
  /** Resolves with an evaluation of an Object flag */
  resolveObjectEvaluation<T extends JsonValue>(
    flagKey: string,
    defaultValue: T,
    context: EvaluationContext,
  ): Promise<ResolutionDetails<T>> {
    return Promise.resolve(this.evaluate(flagKey, defaultValue, context));
  }
  /** Resolves with an evaluation of a String flag */
  resolveStringEvaluation(
    flagKey: string,
    defaultValue: string,
    context: EvaluationContext,
  ): Promise<ResolutionDetails<string>> {
    return Promise.resolve(this.evaluate(flagKey, defaultValue, context));
  }
}

function hasKey<K extends string>(obj: object, key: K): obj is { [P in K]: unknown } {
  return key in obj;
}

function isAssignableTo<T>(value: unknown, schema: T): value is T {
  if (typeof schema !== typeof value) return false;
  if (typeof value === 'object' && typeof schema === 'object') {
    if (schema === null) return value === null;
    if (Array.isArray(schema)) {
      if (!Array.isArray(value)) return false;
      if (schema.length == 0) return true;
      return value.every(item => isAssignableTo(item, schema[0]));
    }
    for (const [key, schemaValue] of Object.entries(schema)) {
      if (!hasKey(value!, key)) return false;
      if (!isAssignableTo(value[key], schemaValue)) return false;
    }
  }
  return true;
}
