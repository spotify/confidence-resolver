import type { EvaluationContext, JsonValue, Provider, ProviderMetadata, ProviderStatus } from '@openfeature/server-sdk';
import {
  ErrorCode as ProtoErrorCode,
  ResolveReason,
  ResolutionDetails as ProtoDetails,
} from './proto/state_module/api';
import type { Materialization_Record, ResolveFlagsRequest } from './proto/state_module/api';
import { SetResolverStateRequest } from './proto/confidence/wasm/messages';
import { Fetch, withLogging, withResponse, withRetry, withRouter, withStallTimeout, withTimeout } from './fetch';
import {
  bytesFromBase64,
  castStringToEnum,
  hexToBytes,
  scheduleWithFixedInterval,
  timeoutSignal,
  TimeUnit,
} from './util';
import { sha256Hex } from './hash';
import { getLogger } from './logger';
import {
  ConfidenceRemoteMaterializationStore,
  type MaterializationStore,
  materializationRecordsToReadOps,
  materializationRecordsToWriteOps,
  readResultsToMaterializationRecords,
} from './materialization';
import { ResolveHandle, StateModule } from './StateModule';
import type { ResolvedFlag as V1ResolvedFlag } from './proto/confidence/flags/resolver/v1/api';
import FlagBundleType, * as FlagBundle from './flag-bundle';
import { CONFIDENCE_PROVIDER_NAME, ErrorCode, ResolutionDetails, ResolutionReason } from './types';

type FlagBundle = FlagBundleType;

const logger = getLogger('state-module-provider');

export const DEFAULT_INITIALIZE_TIMEOUT = 30_000;
export const DEFAULT_STATE_INTERVAL = 30_000;
export const DEFAULT_FLUSH_INTERVAL = 15_000;

/** POC: compiled state modules are served by a local build server, not the CDN. */
export const DEFAULT_STATE_BASE_URL = 'http://localhost:8080/module-state';

const LOGS_URL = 'https://resolver.confidence.dev/v1/clientFlagLogs:write';

export interface StateModuleProviderOptions {
  flagClientSecret: string;
  /** Hex-encoded AES-256 encryption key for decrypting the served state. */
  encryptionKey?: string;
  initializeTimeout?: number;
  /** Interval in milliseconds between state polling updates. Defaults to 30000ms. */
  stateUpdateInterval?: number;
  /** Interval in milliseconds between log flushes. Defaults to 15000ms. */
  flushInterval?: number;
  /** Where compiled state modules are served from. Defaults to {@link DEFAULT_STATE_BASE_URL}. */
  stateBaseUrl?: string;
  fetch?: typeof fetch;
  materializationStore?: MaterializationStore | 'CONFIDENCE_REMOTE_STORE';
}

/**
 * OpenFeature provider backed by a compiled state module.
 *
 * Holds no resolver abstraction and does no host-side evaluation of its own:
 * the module's `evaluate` export owns key splitting, path walking, type
 * checking and default merging, and hands back a ResolutionDetails this
 * provider only reshapes into the OpenFeature type. See plans/wasm-evaluate.md
 * in the state-to-wasm repo for the semantics — notably, they differ from the
 * old host-side implementation on client-default matches and absent struct
 * fields.
 *
 * {@link resolve} is the exception, and deliberately so: bundles are evaluated
 * in the browser, where there is no module to ask.
 */
export class StateModuleProvider implements Provider {
  readonly metadata: ProviderMetadata = {
    name: CONFIDENCE_PROVIDER_NAME,
  };
  status: ProviderStatus = castStringToEnum<ProviderStatus>('NOT_READY');

  private readonly main = new AbortController();
  private readonly fetch: Fetch;
  private readonly stateBaseUrl: string;
  private readonly stateUpdateInterval: number;
  private readonly flushInterval: number;
  private readonly materializationStore: MaterializationStore | null;
  private readonly inFlightLogs = new Set<Promise<void>>();
  private module: StateModule | null = null;
  private stateEtag: string | null = null;

  constructor(private readonly options: StateModuleProviderOptions) {
    this.stateBaseUrl = options.stateBaseUrl ?? DEFAULT_STATE_BASE_URL;
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
          [`${this.stateBaseUrl}/*`]: [
            withRetry({
              maxAttempts: Infinity,
              baseInterval: 500,
              maxInterval: this.stateUpdateInterval,
            }),
            withStallTimeout(1 * TimeUnit.SECOND),
          ],
          'https://resolver.confidence.dev/*': [
            withRouter({
              '*/v1/materialization:readMaterializedOperations': [
                withRetry({ maxAttempts: 3, baseInterval: 100 }),
                withTimeout(0.5 * TimeUnit.SECOND),
              ],
              '*/v1/materialization:writeMaterializedOperations': [
                withRetry({ maxAttempts: 3, baseInterval: 100 }),
                withTimeout(0.5 * TimeUnit.SECOND),
              ],
              '*/v1/clientFlagLogs:write': [
                withRetry({ maxAttempts: 3, baseInterval: 500 }),
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
    if (options.materializationStore === 'CONFIDENCE_REMOTE_STORE') {
      this.materializationStore = new ConfidenceRemoteMaterializationStore(
        options.flagClientSecret,
        this.fetch,
        this.main.signal,
      );
    } else {
      this.materializationStore = options.materializationStore ?? null;
    }
  }

  async initialize(): Promise<void> {
    const signal = this.main.signal;
    const initialUpdateSignal = AbortSignal.any([
      signal,
      timeoutSignal(this.options.initializeTimeout ?? DEFAULT_INITIALIZE_TIMEOUT),
    ]);
    try {
      await this.updateState(initialUpdateSignal);
      scheduleWithFixedInterval(() => this.flush(), this.flushInterval, { maxConcurrent: 3, signal });
      scheduleWithFixedInterval(s => this.updateState(s), this.stateUpdateInterval, { signal });
      this.status = castStringToEnum<ProviderStatus>('READY');
    } catch (e: unknown) {
      this.status = castStringToEnum<ProviderStatus>('ERROR');
      throw e;
    }
  }

  async onClose(): Promise<void> {
    await this.flush();
    this.main.abort();
  }

  async evaluate<T extends JsonValue>(
    flagKey: string,
    defaultValue: T,
    context: EvaluationContext,
  ): Promise<ResolutionDetails<T>> {
    const module = this.module;
    if (!module) {
      return errorDetails(defaultValue, ErrorCode.PROVIDER_NOT_READY, 'Provider not initialized');
    }
    const { _confidence_skip_apply, ...cleanContext } = context;

    const [flagName] = flagKey.split('.', 1);
    let handle: ResolveHandle;
    try {
      handle = await this.startResolve(module, cleanContext, [flagName], _confidence_skip_apply !== true);
    } catch (err) {
      return errorDetails(defaultValue, ErrorCode.GENERAL, String(err));
    }

    try {
      return toOpenFeature(handle.evaluate({ flagKey, defaultValue }), defaultValue);
    } catch (err) {
      // only reachable on a module-level failure (OOM); evaluation errors are in band
      return errorDetails(defaultValue, ErrorCode.GENERAL, String(err));
    } finally {
      handle.close();
    }
  }

  /**
   * Resolves a set of flags into a bundle for client-side evaluation.
   *
   * The bundle carries resolved values, not targeting rules, so it is safe to
   * serialize to a browser — which is why the client hooks evaluate it in JS
   * rather than through the module: shipping a state module to the client
   * would ship every flag's targeting with it.
   */
  async resolve(context: EvaluationContext, flagNames: string[], apply = false): Promise<FlagBundle> {
    const module = this.module;
    if (!module) return FlagBundle.error(ErrorCode.PROVIDER_NOT_READY, 'Provider not initialized');
    try {
      const handle = await this.startResolve(module, context, flagNames, apply);
      try {
        const resolved = handle.decode().resolved;
        if (!resolved) throw new Error('resolve suspended without a materialization store');
        return FlagBundle.create({
          resolveId: '',
          resolveToken: resolved.resolveToken,
          // wire-compatible with the v1 ResolvedFlag this bundle is built from
          resolvedFlags: resolved.resolvedFlags as V1ResolvedFlag[],
        });
      } finally {
        handle.close();
      }
    } catch (err) {
      logger.warn('Resolve failed for [%s]', flagNames.join(', '), err);
      return FlagBundle.error(ErrorCode.GENERAL, String(err));
    }
  }

  /**
   * Applies a previously resolved flag, logging that it was used/exposed.
   * @param resolveToken - Base64-encoded resolve token from an earlier resolve
   * @param flagName - Name of the flag to apply
   */
  applyFlag(resolveToken: string, flagName: string): void {
    const module = this.module;
    if (!module) throw new Error('Provider not initialized');
    const now = timestamp(Date.now());
    module.applyFlags({
      appliedFlags: [{ flag: `flags/${flagName}`, applyTime: now }],
      resolveToken: bytesFromBase64(resolveToken),
      sendTime: now,
    });
  }

  async updateState(signal?: AbortSignal): Promise<void> {
    const hashHex = await sha256Hex(this.options.flagClientSecret);
    const { encryptionKey } = this.options;
    const url = `${this.stateBaseUrl}/${encryptionKey ? `${hashHex}.enc` : hashHex}`;

    const headers = new Headers();
    if (this.stateEtag) headers.set('If-None-Match', this.stateEtag);
    const resp = await this.fetch(url, { headers, signal });
    if (resp.status === 304) return;
    if (!resp.ok) {
      throw new Error(`Failed to fetch state: ${resp.status} ${resp.statusText}`);
    }
    this.stateEtag = resp.headers.get('etag');

    const bytes = new Uint8Array(await resp.arrayBuffer());
    const plaintext = encryptionKey ? await decryptAesGcm(bytes, hexToBytes(encryptionKey)) : bytes;
    // the state payload wraps the compiled module in a SetResolverStateRequest
    const { state } = SetResolverStateRequest.decode(plaintext);
    const compiled = new WebAssembly.Module(
      state.buffer.slice(state.byteOffset, state.byteOffset + state.byteLength) as ArrayBuffer,
    );

    // the outgoing module owns telemetry the new one can't see, so drain it
    // before dropping the reference
    await this.flush();
    this.module = new StateModule(compiled, { onLogs: logs => this.sendLogs(logs.copy()) });
  }

  /** Delivers pending telemetry and waits for the sends to complete. */
  async flush(): Promise<void> {
    this.module?.flushLogs();
    await Promise.allSettled([...this.inFlightLogs]);
  }

  /** Resolves the named flags, leaving the response in module memory. */
  private async startResolve(
    module: StateModule,
    context: EvaluationContext,
    flagNames: string[],
    apply: boolean,
  ): Promise<ResolveHandle> {
    const request: ResolveFlagsRequest = {
      flags: flagNames.map(name => `flags/${name}`),
      evaluationContext: convertEvaluationContext(context),
      apply,
      clientSecret: this.options.flagClientSecret,
    };

    // discovery mode: flags needing materializations resolve with
    // MATERIALIZATION_NOT_SUPPORTED, which evaluate turns into an error
    if (!this.materializationStore) return module.resolve(request);

    const started = module.resolveStart(request);
    const response = started.decode();
    if (response.resolved) {
      this.writeMaterializations(response.resolved.materializationToWrite);
      return started;
    }
    started.close();

    const { processId, materializationToRead } = response.suspended!;
    let records: Materialization_Record[];
    try {
      const readResults = await this.readMaterializations(materializationRecordsToReadOps(materializationToRead));
      records = readResultsToMaterializationRecords(readResults);
    } catch (err) {
      module.resolveDiscard(processId);
      throw err;
    }
    const resumed = module.resolveResume(processId, records);
    this.writeMaterializations(resumed.decode().resolved?.materializationToWrite ?? []);
    return resumed;
  }

  private sendLogs(body: Uint8Array): void {
    const send = this.postLogs(body).finally(() => this.inFlightLogs.delete(send));
    this.inFlightLogs.add(send);
  }

  private async postLogs(body: Uint8Array): Promise<void> {
    try {
      const response = await this.fetch(LOGS_URL, {
        method: 'post',
        signal: this.main.signal,
        headers: {
          'Content-Type': 'application/x-protobuf',
          Authorization: `ClientSecret ${this.options.flagClientSecret}`,
        },
        body: body as Uint8Array<ArrayBuffer>,
      });
      if (!response.ok) {
        logger.error(`Failed to write flag logs: ${response.status} ${response.statusText} - ${await response.text()}`);
      }
    } catch (err) {
      logger.warn('Failed to send flag logs', err);
    }
  }

  private async readMaterializations(
    readOps: MaterializationStore.ReadOp[],
  ): Promise<MaterializationStore.ReadResult[]> {
    const store = this.materializationStore;
    if (!store?.readMaterializations) throw new Error('Read materialization not supported');
    return store.readMaterializations(readOps);
  }

  private writeMaterializations(records: Materialization_Record[]): void {
    if (records.length === 0) return;
    const store = this.materializationStore;
    if (!store?.writeMaterializations) throw new Error('Write materialization not supported');
    store.writeMaterializations(materializationRecordsToWriteOps(records)).catch(e => {
      logger.warn('Failed to write materialization', e);
    });
  }

  /** Resolves with an evaluation of a Boolean flag */
  resolveBooleanEvaluation(
    flagKey: string,
    defaultValue: boolean,
    context: EvaluationContext,
  ): Promise<ResolutionDetails<boolean>> {
    return this.evaluate(flagKey, defaultValue, context);
  }
  /** Resolves with an evaluation of a Number flag */
  resolveNumberEvaluation(
    flagKey: string,
    defaultValue: number,
    context: EvaluationContext,
  ): Promise<ResolutionDetails<number>> {
    return this.evaluate(flagKey, defaultValue, context);
  }
  /** Resolves with an evaluation of an Object flag */
  resolveObjectEvaluation<T extends JsonValue>(
    flagKey: string,
    defaultValue: T,
    context: EvaluationContext,
  ): Promise<ResolutionDetails<T>> {
    return this.evaluate(flagKey, defaultValue, context);
  }
  /** Resolves with an evaluation of a String flag */
  resolveStringEvaluation(
    flagKey: string,
    defaultValue: string,
    context: EvaluationContext,
  ): Promise<ResolutionDetails<string>> {
    return this.evaluate(flagKey, defaultValue, context);
  }
}

// the module's contract: the reason is honest even on some error paths, so
// OpenFeature's reason is ERROR whenever an error code is set
function toOpenFeature<T extends JsonValue>(details: ProtoDetails, defaultValue: T): ResolutionDetails<T> {
  const errorCode = toErrorCode(details.errorCode);
  if (errorCode) {
    logger.warn('Flag evaluation failed: %s %s', errorCode, details.errorMessage);
    return {
      reason: 'ERROR',
      value: (details.value ?? defaultValue) as T,
      errorCode,
      errorMessage: details.errorMessage,
      shouldApply: false,
    };
  }
  return {
    reason: toReason(details.reason),
    value: details.value as T,
    // passed through as the full resource name, as the host-side provider does
    variant: details.variant,
    shouldApply: details.shouldApply,
  };
}

function errorDetails<T extends JsonValue>(value: T, errorCode: ErrorCode, errorMessage: string): ResolutionDetails<T> {
  logger.warn('Flag evaluation failed: %s %s', errorCode, errorMessage);
  return { reason: 'ERROR', value, errorCode, errorMessage, shouldApply: false };
}

function toErrorCode(code: ProtoErrorCode): ErrorCode | undefined {
  switch (code) {
    case ProtoErrorCode.ERROR_CODE_PROVIDER_NOT_READY:
      return ErrorCode.PROVIDER_NOT_READY;
    case ProtoErrorCode.ERROR_CODE_PROVIDER_FATAL:
      return ErrorCode.PROVIDER_FATAL;
    case ProtoErrorCode.ERROR_CODE_FLAG_NOT_FOUND:
      return ErrorCode.FLAG_NOT_FOUND;
    case ProtoErrorCode.ERROR_CODE_TYPE_MISMATCH:
      return ErrorCode.TYPE_MISMATCH;
    case ProtoErrorCode.ERROR_CODE_UNSPECIFIED:
      return undefined;
    default:
      return ErrorCode.GENERAL;
  }
}

function toReason(reason: ResolveReason): ResolutionReason {
  switch (reason) {
    case ResolveReason.RESOLVE_REASON_MATCH:
      return 'MATCH';
    case ResolveReason.RESOLVE_REASON_NO_SEGMENT_MATCH:
      return 'NO_SEGMENT_MATCH';
    case ResolveReason.RESOLVE_REASON_NO_TREATMENT_MATCH:
      return 'NO_TREATMENT_MATCH';
    case ResolveReason.RESOLVE_REASON_FLAG_ARCHIVED:
      return 'FLAG_ARCHIVED';
    case ResolveReason.RESOLVE_REASON_TARGETING_KEY_ERROR:
      return 'TARGETING_KEY_ERROR';
    case ResolveReason.RESOLVE_REASON_MATERIALIZATION_NOT_SUPPORTED:
      return 'MATERIALIZATION_NOT_SUPPORTED';
    case ResolveReason.RESOLVE_REASON_ERROR:
    case ResolveReason.RESOLVE_REASON_UNRECOGNIZED_TARGETING_RULE:
      return 'ERROR';
    default:
      return 'UNSPECIFIED';
  }
}

function convertEvaluationContext({ targetingKey: targeting_key, ...rest }: EvaluationContext): {
  [key: string]: any;
} {
  return { targeting_key, ...rest };
}

function timestamp(millis: number): { seconds: number; nanos: number } {
  return { seconds: Math.floor(millis / 1000), nanos: (millis % 1000) * 1_000_000 };
}

async function decryptAesGcm(data: Uint8Array, rawKey: Uint8Array): Promise<Uint8Array> {
  const NONCE_LEN = 12;
  if (data.length < NONCE_LEN) {
    throw new Error('Encrypted state too short (missing nonce)');
  }
  const iv = data.buffer.slice(data.byteOffset, data.byteOffset + NONCE_LEN) as ArrayBuffer;
  const ciphertext = data.buffer.slice(data.byteOffset + NONCE_LEN, data.byteOffset + data.byteLength) as ArrayBuffer;
  const key = await crypto.subtle.importKey('raw', rawKey.buffer as ArrayBuffer, 'AES-GCM', false, ['decrypt']);
  return new Uint8Array(await crypto.subtle.decrypt({ name: 'AES-GCM', iv }, key, ciphertext));
}
