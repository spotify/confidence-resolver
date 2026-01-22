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
import { ResolveFlagsResponse } from './proto/confidence/flags/resolver/v1/api';
import { ResolveWithStickyRequest } from './proto/confidence/wasm/wasm_api';
import { SdkId, ResolveReason } from './proto/confidence/flags/resolver/v1/types';
import { VERSION } from './version';
import { Fetch, withLogging, withResponse, withRetry, withRouter, withStallTimeout, withTimeout } from './fetch';
import { castStringToEnum, hasKey, scheduleWithFixedInterval, timeoutSignal, TimeUnit } from './util';
import { LocalResolver } from './LocalResolver';
import { sha256Hex } from './hash';
import { getLogger } from './logger';
import {
  ConfidenceRemoteMaterializationStore,
  MaterializationStore,
  readOpsFromProto,
  readResultToProto,
  writeOpsFromProto,
} from './materialization';
import {
  ReadOperationsRequest,
  ReadOperationsResult,
  WriteOperationsRequest,
} from './proto/confidence/flags/resolver/v1/internal_api';
import { SetResolverStateRequest } from './proto/confidence/wasm/messages';

const logger = getLogger('provider');

export const DEFAULT_INITIALIZE_TIMEOUT = 30_000;
export const DEFAULT_STATE_INTERVAL = 30_000;
export const DEFAULT_FLUSH_INTERVAL = 10_000;
export interface ProviderOptions {
  flagClientSecret: string;
  initializeTimeout?: number;
  /** Interval in milliseconds between state polling updates. Defaults to 30000ms. */
  stateUpdateInterval?: number;
  /** Interval in milliseconds between log flushes. Defaults to 10000ms. */
  flushInterval?: number;
  fetch?: typeof fetch;
  materializationStore?: MaterializationStore | 'CONFIDENCE_REMOTE_STORE';
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
  private readonly stateUpdateInterval: number;
  private readonly flushInterval: number;
  private readonly materializationStore: MaterializationStore | null;
  private stateEtag: string | null = null;

  private get resolver(): LocalResolver {
    if (this.resolverOrPromise instanceof Promise) {
      throw new Error('Resolver not ready');
    }
    return this.resolverOrPromise;
  }

  // TODO Maybe pass in a resolver factory, so that we can initialize it in initialize and transition to fatal if not.
  constructor(private resolverOrPromise: LocalResolver | Promise<LocalResolver>, private options: ProviderOptions) {
    this.stateUpdateInterval = options.stateUpdateInterval ?? DEFAULT_STATE_INTERVAL;
    if (!Number.isInteger(this.stateUpdateInterval) || this.stateUpdateInterval < 1000) {
      throw new Error(`stateUpdateInterval must be an integer >= 1000 (1s), currently: ${this.stateUpdateInterval}`);
    }
    this.flushInterval = options.flushInterval ?? DEFAULT_FLUSH_INTERVAL;
    if (!Number.isInteger(this.flushInterval) || this.flushInterval < 1000) {
      throw new Error(`flushInterval must be an integer >= 1000 (1s), currently: ${this.flushInterval}`);
    }
    this.fetch = Fetch.create(
      [
        withRouter({
          'https://confidence-resolver-state-cdn.spotifycdn.com/*': [
            withRetry({
              maxAttempts: Infinity,
              baseInterval: 500,
              maxInterval: this.stateUpdateInterval,
            }),
            withStallTimeout(500),
          ],
          'https://resolver.confidence.dev/*': [
            withRouter({
              '*/v1/materialization:readMaterializedOperations': [
                withRetry({
                  maxAttempts: 3,
                  baseInterval: 100,
                }),
                withTimeout(0.5 * TimeUnit.SECOND),
              ],
              '*/v1/materialization:writeMaterializedOperations': [
                withRetry({
                  maxAttempts: 3,
                  baseInterval: 100,
                }),
                withTimeout(0.5 * TimeUnit.SECOND),
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
    if (options.materializationStore) {
      if (options.materializationStore === 'CONFIDENCE_REMOTE_STORE') {
        this.materializationStore = new ConfidenceRemoteMaterializationStore(
          options.flagClientSecret,
          this.fetch,
          this.main.signal,
        );
      } else {
        this.materializationStore = options.materializationStore;
      }
    } else {
      this.materializationStore = null;
    }
  }

  async initialize(context?: EvaluationContext): Promise<void> {
    // TODO validate options and switch to fatal.
    const signal = this.main.signal;
    const initialUpdateSignal = AbortSignal.any([
      signal,
      timeoutSignal(this.options.initializeTimeout ?? DEFAULT_INITIALIZE_TIMEOUT),
    ]);
    try {
      this.resolverOrPromise = await this.resolverOrPromise;
      // TODO set schedulers irrespective of failure
      // TODO if 403 here,
      await this.updateState(initialUpdateSignal);
      scheduleWithFixedInterval(signal => this.flush(signal), this.flushInterval, { maxConcurrent: 3, signal });
      // TODO Better with fixed delay so we don't do a double fetch when we're behind. Alt, skip if in progress
      scheduleWithFixedInterval(signal => this.updateState(signal), this.stateUpdateInterval, { signal });
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
    try {
      const [flagName, ...path] = flagKey.split('.');

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
        failFastOnSticky: false,
        notProcessSticky: false,
      };

      const response = await this.resolveWithSticky(stickyRequest);

      return this.extractValue(response.resolvedFlags[0], flagName, path, defaultValue);
    } catch (e) {
      logger.warn(`Flag evaluation for '${flagKey}' failed`, e);
      return {
        value: defaultValue,
        reason: 'ERROR',
        errorCode: castStringToEnum<ErrorCode>('GENERAL'),
        errorMessage: String(e),
      };
    } finally {
      this.flushAssigned();
    }
  }

  private async resolveWithSticky(stickyRequest: ResolveWithStickyRequest): Promise<ResolveFlagsResponse> {
    let stickyResponse = this.resolver.resolveWithSticky(stickyRequest);

    if (stickyResponse.readOpsRequest) {
      const { results: materializations } = await this.readMaterializations(stickyResponse.readOpsRequest);
      stickyResponse = this.resolver.resolveWithSticky({ ...stickyRequest, materializations });
    }

    if (!stickyResponse.success) {
      // this shouldn't happen with failFast = false. Although it _could_ happen if the state changed and added a new read
      throw new Error('Missing materializations');
    }

    const { materializationUpdates: storeVariantOp, response: resolveResponse } = stickyResponse.success;
    if (storeVariantOp.length) {
      // TODO should this be awaited?
      this.writeMaterializations({ storeVariantOp });
    }
    return ResolveFlagsResponse.create(resolveResponse);
  }

  /**
   * Extract and validate the value from a resolved flag.
   */
  private extractValue<T>(flag: any, flagName: string, path: string[], defaultValue: T): ResolutionDetails<T> {
    if (!flag) {
      return {
        value: defaultValue,
        reason: 'ERROR',
        errorCode: castStringToEnum<ErrorCode>('FLAG_NOT_FOUND'),
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
          errorCode: castStringToEnum<ErrorCode>('TYPE_MISMATCH'),
        };
      }
      value = value[step];
    }

    if (!isAssignableTo(value, defaultValue)) {
      return {
        value: defaultValue,
        reason: 'ERROR',
        errorCode: castStringToEnum<ErrorCode>('TYPE_MISMATCH'),
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

  private async readMaterializations(readOpsReq: ReadOperationsRequest): Promise<ReadOperationsResult> {
    const materializationStore = this.materializationStore;
    if (materializationStore && typeof materializationStore.readMaterializations === 'function') {
      const result = await materializationStore.readMaterializations(readOpsFromProto(readOpsReq));
      return readResultToProto(result);
    }
    throw new Error('Read materialization not supported');
  }

  private writeMaterializations(writeOpsRequest: WriteOperationsRequest): void {
    const materializationStore = this.materializationStore;
    if (materializationStore && typeof materializationStore.writeMaterializations === 'function') {
      materializationStore.writeMaterializations(writeOpsFromProto(writeOpsRequest)).catch(e => {
        logger.warn('Failed to write materialization', e);
      });
      return;
    }
    throw new Error('Write materialization not supported');
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
