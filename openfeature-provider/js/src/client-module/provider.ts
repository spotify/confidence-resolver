import type {
  EvaluationContext,
  JsonValue,
  Provider,
  ProviderMetadata,
  ProviderStatus,
  ServerProviderEvents,
} from '@openfeature/server-sdk';
import {
  ErrorCode as WasmErrorCode,
  ResolutionDetails as ModuleResolutionDetails,
} from '../proto/confidence/flags/wasm/v1/api';
import { ResolveReason } from '../proto/confidence/flags/resolver/v1/types';
import { ErrorCode, type ResolutionDetails as LegacyResolutionDetails } from '../types';
import { VERSION } from '../version';
import { ProviderEvents } from '../ProviderEvents';
import { abortableSleep, castStringToEnum, scheduleWithFixedInterval, timeoutSignal } from '../util';
import { validateEncryptionKey } from '../crypto';
import { ConfidenceRemoteMaterializationStore, type MaterializationStore } from '../materialization';
import FlagBundleDefault, * as FlagBundleModule from '../flag-bundle';
import { DEFAULT_MODULE_BASE_URL, DEFAULT_MODULE_UPDATE_INTERVAL, ModuleClient } from './module-client';
import {
  DEFAULT_INGESTION_BASE_URL,
  ModuleRuntime,
  ResolveProcessError,
  RuntimeNotReadyError,
  type ResolveSession,
} from './module-runtime';
import { getLogger } from '../logger';

type FlagBundle = FlagBundleDefault;
const logger = getLogger('module-provider');

export const DEFAULT_INITIALIZE_TIMEOUT = 30_000;
export const DEFAULT_FLUSH_INTERVAL = 15_000;
/** Poll cadence while no module has been accepted yet. */
export const NOT_READY_MODULE_INTERVAL = 1_000;

const PROVIDER_NAME = 'ConfidenceModuleProvider';

/** OpenFeature-shaped evaluation details. */
export interface ProviderResolutionDetails<T> {
  value: T;
  reason: string;
  variant?: string;
  errorCode?: ErrorCode;
  errorMessage?: string;
}

export interface ModuleProviderOptions {
  /** The credential's client secret; also derives the module path. */
  flagClientSecret: string;
  /** Hex-encoded AES-256 encryption key paired with the credential. */
  encryptionKey: string;
  /** Total retry budget in ms for the initial module load. Default 30_000. */
  initializeTimeout?: number;
  /** Interval in ms between module refresh polls. Default 30_000. */
  moduleUpdateInterval?: number;
  /** Interval in ms between flag-log flushes. Default 15_000. */
  flushInterval?: number;
  /** Custom fetch implementation. */
  fetch?: typeof fetch;
  /** Sticky assignments and materialized segments. */
  materializationStore?: MaterializationStore | 'CONFIDENCE_REMOTE_STORE';
  /** Module CDN base URL. Default: the production module CDN. */
  moduleBaseUrl?: string;
  /** Ingestion base URL for log delivery. Default: the regionless ingestion host. */
  ingestionBaseUrl?: string;
  /** Instance pool sizing. */
  maxInstances?: number;
  warmInstances?: number;
  /** Disable exposure collection for every evaluation through this provider. */
  disableExposureCollection?: boolean;
}

/** Map the module's evaluation result onto OpenFeature (guide section 9). */
function mapModuleDetails<T extends JsonValue>(
  details: ModuleResolutionDetails,
  defaultValue: T,
): ProviderResolutionDetails<T> {
  const value = (details.value !== undefined ? details.value : defaultValue) as T;
  if (details.errorCode !== WasmErrorCode.ERROR_CODE_UNSPECIFIED) {
    return {
      // OpenFeature: on error the provider returns the default value.
      value: defaultValue,
      reason: 'ERROR',
      errorCode: mapModuleErrorCode(details.errorCode),
      errorMessage: details.errorMessage || undefined,
    };
  }
  return {
    value,
    reason: mapModuleReason(details.reason),
    variant: details.variant || undefined,
  };
}

function mapModuleReason(reason: ResolveReason): string {
  switch (reason) {
    case ResolveReason.RESOLVE_REASON_MATCH:
      return 'TARGETING_MATCH';
    case ResolveReason.RESOLVE_REASON_NO_SEGMENT_MATCH:
    case ResolveReason.RESOLVE_REASON_NO_TREATMENT_MATCH:
      return 'DEFAULT';
    case ResolveReason.RESOLVE_REASON_FLAG_ARCHIVED:
      return 'DISABLED';
    default:
      return 'UNKNOWN';
  }
}

function mapModuleErrorCode(code: WasmErrorCode): ErrorCode {
  switch (code) {
    case WasmErrorCode.ERROR_CODE_PROVIDER_NOT_READY:
      return ErrorCode.PROVIDER_NOT_READY;
    case WasmErrorCode.ERROR_CODE_FLAG_NOT_FOUND:
      return ErrorCode.FLAG_NOT_FOUND;
    case WasmErrorCode.ERROR_CODE_TYPE_MISMATCH:
      return ErrorCode.TYPE_MISMATCH;
    default:
      return ErrorCode.GENERAL;
  }
}

/** Map the host-side (flag-bundle) evaluation fallback onto OpenFeature. */
function mapLegacyDetails<T extends JsonValue>(details: LegacyResolutionDetails<T>): ProviderResolutionDetails<T> {
  if (details.errorCode) {
    return {
      value: details.value,
      reason: 'ERROR',
      errorCode: details.errorCode,
      errorMessage: details.errorMessage,
    };
  }
  return {
    value: details.value,
    reason: mapModuleReason(reasonStringToEnum(details.reason)),
    variant: details.variant || undefined,
  };
}

function reasonStringToEnum(reason: string): ResolveReason {
  switch (reason) {
    case 'MATCH':
      return ResolveReason.RESOLVE_REASON_MATCH;
    case 'NO_SEGMENT_MATCH':
      return ResolveReason.RESOLVE_REASON_NO_SEGMENT_MATCH;
    case 'NO_TREATMENT_MATCH':
      return ResolveReason.RESOLVE_REASON_NO_TREATMENT_MATCH;
    case 'FLAG_ARCHIVED':
      return ResolveReason.RESOLVE_REASON_FLAG_ARCHIVED;
    case 'TARGETING_KEY_ERROR':
    case 'ERROR':
      return ResolveReason.RESOLVE_REASON_ERROR;
    default:
      return ResolveReason.RESOLVE_REASON_UNSPECIFIED;
  }
}

function convertEvaluationContext({
  targetingKey: targeting_key,
  ...rest
}: EvaluationContext): Record<string, unknown> {
  return { targeting_key, ...rest };
}

/**
 * OpenFeature provider hosting one credential's compiled client module:
 * fetches and verifies the artifact, resolves flags locally through pooled
 * WASM instances, evaluates via the module's evaluation step, applies deferred
 * exposures, and forwards the module's logs to the ingestion API with the
 * wrapper's token.
 *
 * The public surface mirrors the state-based ConfidenceServerProviderLocal:
 * `resolve()` bundles flags for React Server Components (exposures deferred
 * until `applyFlag`), `evaluate()`-backed `resolve*Evaluation` for the
 * OpenFeature client, and `getPrometheusMetrics()` for scraping.
 */
export class ConfidenceModuleProvider implements Provider {
  readonly metadata: ProviderMetadata = { name: PROVIDER_NAME };
  status: ProviderStatus = castStringToEnum<ProviderStatus>('NOT_READY');
  readonly events: ProviderEvents = new ProviderEvents(PROVIDER_NAME);

  private readonly main = new AbortController();
  private readonly options: ModuleProviderOptions;
  private readonly runtime: ModuleRuntime;
  private readonly initializeTimeout: number;
  private readonly moduleUpdateInterval: number;
  private readonly flushInterval: number;

  constructor(options: ModuleProviderOptions) {
    validateEncryptionKey(options.encryptionKey);
    if (!options.flagClientSecret) {
      throw new Error('flagClientSecret is required');
    }
    for (const [name, value] of [
      ['moduleUpdateInterval', options.moduleUpdateInterval ?? DEFAULT_MODULE_UPDATE_INTERVAL],
      ['flushInterval', options.flushInterval ?? DEFAULT_FLUSH_INTERVAL],
    ] as const) {
      if (!Number.isInteger(value) || value < 1000) {
        throw new Error(`${name} must be an integer >= 1000 (1s), currently: ${value}`);
      }
    }
    this.options = { ...options };
    this.initializeTimeout = options.initializeTimeout ?? DEFAULT_INITIALIZE_TIMEOUT;
    this.moduleUpdateInterval = options.moduleUpdateInterval ?? DEFAULT_MODULE_UPDATE_INTERVAL;
    this.flushInterval = options.flushInterval ?? DEFAULT_FLUSH_INTERVAL;

    const materializationStore =
      options.materializationStore === 'CONFIDENCE_REMOTE_STORE'
        ? new ConfidenceRemoteMaterializationStore(options.flagClientSecret, options.fetch ?? fetch, this.main.signal)
        : options.materializationStore;

    this.runtime = new ModuleRuntime(
      new ModuleClient({
        clientSecret: options.flagClientSecret,
        encryptionKey: options.encryptionKey,
        baseUrl: options.moduleBaseUrl ?? DEFAULT_MODULE_BASE_URL,
        fetch: options.fetch,
      }),
      {
        clientSecret: options.flagClientSecret,
        encryptionKey: options.encryptionKey,
        ingestionBaseUrl: options.ingestionBaseUrl ?? DEFAULT_INGESTION_BASE_URL,
        fetch: options.fetch,
        materializationStore,
        maxInstances: options.maxInstances,
        warmInstances: options.warmInstances,
        sdk: { customId: PROVIDER_NAME, version: VERSION },
      },
    );
  }

  async initialize(_context?: EvaluationContext): Promise<void> {
    const signal = this.main.signal;
    const budget = AbortSignal.any([signal, timeoutSignal(this.initializeTimeout)]);
    let initializationError: Error | undefined;
    try {
      while (true) {
        budget.throwIfAborted();
        const outcome = await this.runtime.refresh(budget);
        if (outcome.type === 'accepted') break;
        logger.info('Module not available yet, retrying:', outcome);
        await abortableSleep(NOT_READY_MODULE_INTERVAL, budget);
      }
      this.status = castStringToEnum<ProviderStatus>('READY');
    } catch (cause) {
      this.status = castStringToEnum<ProviderStatus>('ERROR');
      initializationError = Object.assign(
        new Error(signal.aborted ? 'Provider closed during initialization' : 'Timed out waiting for initial module'),
        { code: 'PROVIDER_NOT_READY', cause },
      );
      logger.warn('Initial module unavailable:', initializationError);
    }

    if (!signal.aborted) {
      scheduleWithFixedInterval(
        async signal => {
          const outcome = await this.runtime.refresh(signal);
          if (this.status !== 'READY' && this.runtime.isReady) {
            this.status = castStringToEnum<ProviderStatus>('READY');
            this.events.emit(castStringToEnum<ServerProviderEvents>('PROVIDER_READY'));
          }
          if (outcome.type === 'failed') {
            logger.warn('Module refresh failed, keeping last good module:', outcome.reason);
          }
        },
        () => (this.runtime.isReady ? this.moduleUpdateInterval : NOT_READY_MODULE_INTERVAL),
        { signal },
      );
      scheduleWithFixedInterval(() => this.runtime.flushLogs(), this.flushInterval, { maxConcurrent: 3, signal });
    }
    if (initializationError) throw initializationError;
  }

  async onClose(): Promise<void> {
    this.main.abort();
    try {
      // Best-effort: flush what is buffered and give in-flight deliveries a
      // bounded chance to finish before the queue closes.
      await this.runtime.drainLogs(1000);
    } catch {
      // provider is shutting down
    }
    await this.runtime.close();
  }

  /**
   * Evaluate one flag, logging exposure immediately. Public surface kept
   * from the state-based provider for direct callers and the React layer.
   */
  async evaluate<T extends JsonValue>(
    flagKey: string,
    defaultValue: T,
    context: EvaluationContext,
  ): Promise<ProviderResolutionDetails<T>> {
    const [flagName] = flagKey.split('.', 1);
    const { _confidence_skip_apply, ...cleanContext } = context;
    const skipApply = this.options.disableExposureCollection === true || _confidence_skip_apply === true;

    if (!this.runtime.isReady) {
      return {
        value: defaultValue,
        reason: 'ERROR',
        errorCode: ErrorCode.PROVIDER_NOT_READY,
        errorMessage: 'Provider is not ready',
      };
    }

    let session: ResolveSession | undefined;
    try {
      session = await this.runtime.resolve({
        flags: [flagName],
        evaluationContext: convertEvaluationContext(cleanContext as EvaluationContext),
        apply: !skipApply,
      });
      if (session.canEvaluate) {
        return mapModuleDetails(session.evaluate(flagKey, defaultValue), defaultValue);
      }
      // Host-side fallback while a module lacks the evaluate export.
      const bundle = FlagBundleModule.create(session.response);
      return mapLegacyDetails(FlagBundleModule.resolve(bundle, flagKey, defaultValue, logger));
    } catch (error) {
      return {
        value: defaultValue,
        reason: 'ERROR',
        errorCode: mapErrorToCode(error),
        errorMessage: error instanceof Error ? error.message : String(error),
      };
    } finally {
      session?.release();
    }
  }

  /**
   * Resolve flags into a bundle for React Server Components. Exposures are
   * deferred: the client applies them later through `applyFlag`.
   */
  async resolve(context: EvaluationContext, flagNames: string[]): Promise<FlagBundle> {
    const session = await this.runtime.resolve({
      flags: flagNames,
      evaluationContext: convertEvaluationContext(context),
      apply: false,
    });
    try {
      return FlagBundleModule.create(session.response);
    } finally {
      session.release();
    }
  }

  /** Apply a previously resolved flag: logs that it was used/exposed. */
  applyFlag(resolveToken: string, flagName: string): void {
    void this.runtime
      .applyFlags({
        resolveToken: FlagBundleModule.decodeToken(resolveToken),
        flags: [{ flag: `flags/${flagName}` }],
      })
      .catch(error => logger.warn('Failed to apply flag:', error));
  }

  /** Resolves with an evaluation of a Boolean flag */
  resolveBooleanEvaluation(
    flagKey: string,
    defaultValue: boolean,
    context: EvaluationContext,
  ): Promise<ProviderResolutionDetails<boolean>> {
    return this.evaluate(flagKey, defaultValue, context);
  }

  /** Resolves with an evaluation of a Numbers flag */
  resolveNumberEvaluation(
    flagKey: string,
    defaultValue: number,
    context: EvaluationContext,
  ): Promise<ProviderResolutionDetails<number>> {
    return this.evaluate(flagKey, defaultValue, context);
  }

  /** Resolves with an evaluation of an Object flag */
  resolveObjectEvaluation<T extends JsonValue>(
    flagKey: string,
    defaultValue: T,
    context: EvaluationContext,
  ): Promise<ProviderResolutionDetails<T>> {
    return this.evaluate(flagKey, defaultValue, context);
  }

  /** Resolves with an evaluation of a String flag */
  resolveStringEvaluation(
    flagKey: string,
    defaultValue: string,
    context: EvaluationContext,
  ): Promise<ProviderResolutionDetails<string>> {
    return this.evaluate(flagKey, defaultValue, context);
  }

  /** Returns a Prometheus metrics snapshot from the module. */
  async getPrometheusMetrics(): Promise<string> {
    return this.runtime.metricsSnapshot(0);
  }

  /** Flush buffered flag logs now. */
  async flush(): Promise<void> {
    this.runtime.flushLogs();
  }
}

function mapErrorToCode(error: unknown): ErrorCode {
  if (error instanceof RuntimeNotReadyError) return ErrorCode.PROVIDER_NOT_READY;
  if (error instanceof ResolveProcessError && error.code === 16) return ErrorCode.PROVIDER_FATAL;
  return ErrorCode.GENERAL;
}
