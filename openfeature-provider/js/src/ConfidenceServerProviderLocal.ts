import type {
  EvaluationContext,
  JsonValue,
  Provider,
  ProviderMetadata,
  ProviderStatus,
  TrackingEventDetails,
} from '@openfeature/server-sdk';
import { ResolveFlagsResponse } from './proto/confidence/flags/resolver/v1/api';
import { ResolveProcessRequest, ResolveProcessResponse } from './proto/confidence/wasm/wasm_api';
import { ResolveReason, SdkId } from './proto/confidence/flags/resolver/v1/types';
import { VERSION } from './version';
import { Fetch, withLogging, withResponse, withRetry, withRouter, withStallTimeout, withTimeout } from './fetch';
import { castStringToEnum, hexToBytes, scheduleWithFixedInterval, timeoutSignal, TimeUnit } from './util';
import type { LocalResolver } from './LocalResolver';
import { sha256Hex } from './hash';
import { getLogger } from './logger';
import {
  ConfidenceRemoteMaterializationStore,
  type MaterializationStore,
  materializationRecordsToReadOps,
  materializationRecordsToWriteOps,
  readResultsToMaterializationRecords,
} from './materialization';
import { SetResolverStateRequest } from './proto/confidence/wasm/messages';
import { ClientResolverState, LogDestination } from './proto/confidence/flags/admin/v1/resolver';
import { IngestFlagLogsRequest, WriteFlagLogsRequest } from './proto/confidence/flags/resolver/v1/internal_api';
import FlagBundleType, * as FlagBundle from './flag-bundle';
import { ErrorCode, ResolutionDetails } from './types';
import type { EventTracker } from './EventWasmTracker';
import { TrackEventRequest, FlushEventsResponse } from './proto/confidence/events/wasm/v1/wasm_api';
import { SdkId as EventsSdkId } from './proto/confidence/events/v1/types';
import { PublishEventsRequest, PublishEventsResponse } from './proto/confidence/events/v1/api';
import { EventError_Reason } from './proto/confidence/events/v1/types';

type FlagBundle = FlagBundleType;
const logger = getLogger('provider');

export const DEFAULT_INITIALIZE_TIMEOUT = 30_000;
export const DEFAULT_STATE_INTERVAL = 30_000;
export const DEFAULT_FLUSH_INTERVAL = 15_000;
/** Upper bound on flush calls during shutdown drain, so a failing publish cannot spin forever. */
const MAX_DRAIN_BATCHES = 100;

/**
 * Configuration for {@link ConfidenceServerProviderLocal.getPrometheusMetrics}.
 *
 * @experimental This API is subject to change.
 */
// eslint-disable-next-line @typescript-eslint/no-empty-interface
export interface SnapshotConfig {}
export interface ProviderOptions {
  flagClientSecret: string;
  /** Hex-encoded AES-256 encryption key for decrypting CDN state. */
  encryptionKey?: string;
  initializeTimeout?: number;
  /** Interval in milliseconds between state polling updates. Defaults to 30000ms. */
  stateUpdateInterval?: number;
  /** Interval in milliseconds between log flushes. Defaults to 15000ms. */
  flushInterval?: number;
  fetch?: typeof fetch;
  materializationStore?: MaterializationStore | 'CONFIDENCE_REMOTE_STORE';
  /**
   * Experimental: enable apply-event deduplication in the WASM resolver —
   * repeated identical assignments within a short TTL window are logged once.
   * Off by default; the API may change.
   */
  enableApplyDedup?: boolean;
  /**
   * Disable exposure/assignment collection for all OpenFeature evaluations
   * through this provider. Use only for exceptional no-exposure modes; resolve
   * logs and telemetry are still sent.
   */
  disableExposureCollection?: boolean;
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
  status: ProviderStatus = castStringToEnum<ProviderStatus>('NOT_READY');

  private readonly main = new AbortController();
  private readonly fetch: Fetch;
  private readonly stateUpdateInterval: number;
  private readonly flushInterval: number;
  private readonly materializationStore: MaterializationStore | null;
  private readonly initLabels: Record<string, string>;
  private initTelemetryState: 'pending' | 'sending' | 'sent' = 'pending';
  private flushSucceeded = 0;
  private flushFailed = 0;
  private eventsPublished = 0;
  private eventBatchesSucceeded = 0;
  private eventBatchesFailed = 0;
  private resolverInstance: LocalResolver | null = null;
  private eventTracker: EventTracker | null = null;
  private stateEtag: string | null = null;
  private logDestinations: LogDestination[] = [];
  private accountId = '';

  private get resolver(): LocalResolver {
    if (!this.resolverInstance) {
      throw new Error('Resolver not ready');
    }
    return this.resolverInstance;
  }

  // TODO Maybe pass in a resolver factory, so that we can initialize it in initialize and transition to fatal if not.
  constructor(
    private readonly resolverOrPromise: LocalResolver | Promise<LocalResolver>,
    private readonly eventTrackerOrPromise: EventTracker | Promise<EventTracker>,
    private options: ProviderOptions,
  ) {
    if (!(resolverOrPromise instanceof Promise)) {
      this.resolverInstance = resolverOrPromise;
    }
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
            withStallTimeout(1 * TimeUnit.SECOND),
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
          'https://epx-flags-logs.experimentation-platform.workers.dev/*': [
            withRetry({
              maxAttempts: 3,
              baseInterval: 500,
            }),
            withTimeout(5 * TimeUnit.SECOND),
          ],
          'https://events.confidence.dev/*': [
            withRetry({
              maxAttempts: 3,
              baseInterval: 500,
            }),
            withTimeout(5 * TimeUnit.SECOND),
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
    this.initLabels = { encryption: options.encryptionKey ? 'true' : 'false' };
  }

  async initialize(context?: EvaluationContext): Promise<void> {
    if (!this.options.encryptionKey) {
      logger.warn(
        'No encryptionKey provided. Falling back to unencrypted state. ' +
          'An encryption key will be required in an upcoming version.',
      );
    }
    const signal = this.main.signal;
    const initialUpdateSignal = AbortSignal.any([
      signal,
      timeoutSignal(this.options.initializeTimeout ?? DEFAULT_INITIALIZE_TIMEOUT),
    ]);
    try {
      this.resolverInstance = await this.resolverOrPromise;
      // TODO set schedulers irrespective of failure
      // TODO if 403 here,
      await this.updateState(initialUpdateSignal);
      scheduleWithFixedInterval(signal => this.flush(signal), this.flushInterval, { maxConcurrent: 3, signal });
      this.eventTracker = await this.eventTrackerOrPromise;
      scheduleWithFixedInterval(signal => this.flushEvents(signal), this.flushInterval, { maxConcurrent: 3, signal });
      // TODO Better with fixed delay so we don't do a double fetch when we're behind. Alt, skip if in progress
      scheduleWithFixedInterval(signal => this.updateState(signal), this.stateUpdateInterval, { signal });
      this.status = castStringToEnum<ProviderStatus>('READY');
    } catch (e: unknown) {
      this.status = castStringToEnum<ProviderStatus>('ERROR');
      // TODO should we swallow this?
      throw e;
    }
  }

  async onClose(): Promise<void> {
    const signal = timeoutSignal(3000);
    try {
      try {
        await this.flush(signal);
      } catch {
        // best-effort: try an init-only request below
      }
      if (this.initTelemetryState !== 'sent') {
        try {
          const request = this.addProviderInitTelemetry(new Uint8Array());
          await this.sendFlagLogs(request, signal);
          this.initTelemetryState = 'sent';
        } catch {
          // best-effort: provider is shutting down
        }
      }
      if (this.eventTracker) {
        try {
          await this.drainEvents(signal);
        } catch {
          // best-effort: provider is shutting down
        }
      }
    } finally {
      this.main.abort();
    }
  }

  /**
   * Drain every buffered event on shutdown. A single flush is capped at the
   * WASM-side byte limit, so one call can leave a backlog behind. Bounded so a
   * failing publish cannot spin forever.
   */
  private async drainEvents(signal?: AbortSignal): Promise<void> {
    if (!this.eventTracker) return;
    for (let i = 0; i < MAX_DRAIN_BATCHES; i++) {
      const batch = this.eventTracker.flushEvents();
      if (!batch.events || batch.events.length === 0) return;
      await this.sendEvents(batch, signal);
    }
    logger.warn(`Event drain hit the ${MAX_DRAIN_BATCHES}-batch limit on shutdown; dropping the rest`);
  }

  track(trackingEventName: string, context?: EvaluationContext, details?: TrackingEventDetails): void {
    if (!this.eventTracker) return;

    const { value, ...customData } = details ?? {};
    const trackRequest: TrackEventRequest = {
      eventName: trackingEventName,
      eventTime: new Date(),
      value,
      context: context ? ConfidenceServerProviderLocal.convertEvaluationContext(context) : undefined,
      data: Object.keys(customData).length > 0 ? customData : undefined,
    };
    try {
      this.eventTracker.trackEvent(trackRequest);
    } catch (err) {
      logger.warn('Failed to track event:', err);
    }
  }

  private async flushEvents(signal?: AbortSignal): Promise<void> {
    if (!this.eventTracker) return;
    const batch = this.eventTracker.flushEvents();
    if (!batch.events || batch.events.length === 0) return;
    await this.sendEvents(batch, signal);
  }

  private async sendEvents(batch: FlushEventsResponse, signal = this.main.signal): Promise<void> {
    const request = PublishEventsRequest.create({
      clientSecret: this.options.flagClientSecret,
      events: batch.events,
      sendTime: new Date(),
      sdk: { id: EventsSdkId.SDK_ID_JS_LOCAL_SERVER_PROVIDER, version: VERSION },
    });
    const body = PublishEventsRequest.encode(request).finish();

    try {
      const response = await this.fetch('https://events.confidence.dev/v1/events:publish', {
        method: 'post',
        signal,
        headers: { 'Content-Type': 'application/x-protobuf' },
        body: body as Uint8Array<ArrayBuffer>,
      });
      if (!response.ok) {
        this.eventBatchesFailed++;
        logger.error(`Failed to send events: ${response.status} ${response.statusText}`);
        return;
      }
      this.eventBatchesSucceeded++;
      this.eventsPublished += batch.events?.length ?? 0;
      const { errors } = PublishEventsResponse.decode(new Uint8Array(await response.arrayBuffer()));
      for (const error of errors) {
        logger.error(
          `Failed to publish event at index ${error.index}: ${EventError_Reason[error.reason]} ${error.message}`,
        );
      }
    } catch (err) {
      this.eventBatchesFailed++;
      logger.warn('Failed to send events:', err);
    }
  }

  async resolve(context: EvaluationContext, flagNames: string[], apply = false): Promise<FlagBundle> {
    const startMs = performance.now();
    let reason = ResolveReason.RESOLVE_REASON_BUNDLE;
    try {
      return await this.resolveFlags(context, flagNames, apply);
    } catch (err) {
      reason = ResolveReason.RESOLVE_REASON_ERROR;
      return FlagBundle.error(ErrorCode.GENERAL, String(err));
    } finally {
      const latencyUs = Math.round((performance.now() - startMs) * 1000);
      try {
        this.resolver.registerResolve({ reason, latencyUs });
      } catch {
        // best-effort telemetry
      }
    }
  }

  private async resolveFlags(context: EvaluationContext, flagNames: string[], apply: boolean): Promise<FlagBundle> {
    const resolveRequest = {
      flags: flagNames.map(name => `flags/${name}`),
      evaluationContext: ConfidenceServerProviderLocal.convertEvaluationContext(context),
      apply,
      clientSecret: this.options.flagClientSecret,
      sdk: {
        id: SdkId.SDK_ID_JS_LOCAL_SERVER_PROVIDER,
        version: VERSION,
      },
    };

    const processRequest: ResolveProcessRequest = this.materializationStore
      ? { deferredMaterializations: resolveRequest }
      : { withoutMaterializations: resolveRequest };

    return FlagBundle.create(await this.resolveProcess(processRequest));
  }

  async evaluate<T extends JsonValue>(
    flagKey: string,
    defaultValue: T,
    context: EvaluationContext,
  ): Promise<ResolutionDetails<T>> {
    const startMs = performance.now();
    try {
      const [flagName] = flagKey.split('.', 1);
      const { _confidence_skip_apply, ...cleanContext } = context;
      // apply=false covers both provider disableExposureCollection and per-eval
      // `_confidence_skip_apply`. Provider disableExposureCollection is also set on the WASM
      // guest via setResolverState so assign/token are skipped entirely;
      // apply=false alone would still mint a deferred token.
      const disableExposureCollection =
        this.options.disableExposureCollection === true || _confidence_skip_apply === true;

      let resolution: FlagBundle;
      try {
        resolution = await this.resolveFlags(cleanContext as EvaluationContext, [flagName], !disableExposureCollection);
      } catch (err) {
        resolution = FlagBundle.error(ErrorCode.GENERAL, String(err));
      }
      const result = FlagBundle.resolve(resolution, flagKey, defaultValue, logger);

      const latencyUs = Math.round((performance.now() - startMs) * 1000);
      let reason: ResolveReason;
      if (resolution.errorCode) {
        reason = ResolveReason.RESOLVE_REASON_ERROR;
      } else {
        const [flagNameForTelemetry] = flagKey.split('.', 1);
        const flagResolution = resolution.flags[flagNameForTelemetry];
        if (flagResolution?.reason === 'MATERIALIZATION_NOT_SUPPORTED') {
          reason = ResolveReason.RESOLVE_REASON_MATERIALIZATION_NOT_SUPPORTED;
        } else if (result.errorCode === ErrorCode.FLAG_NOT_FOUND) {
          reason = ResolveReason.RESOLVE_REASON_FLAG_NOT_FOUND;
        } else if (result.errorCode === ErrorCode.TYPE_MISMATCH) {
          reason = ResolveReason.RESOLVE_REASON_TYPE_MISMATCH;
        } else {
          reason = reasonStringToEnum(result.reason);
        }
      }
      try {
        this.resolver.registerResolve({ reason, latencyUs });
      } catch {
        // best-effort telemetry
      }

      return result;
    } finally {
      if (this.options.disableExposureCollection !== true) {
        this.flushAssigned();
      }
    }
  }

  private async resolveProcess(request: ResolveProcessRequest): Promise<ResolveFlagsResponse> {
    const response = this.resolver.resolveProcess(request);

    if (response.suspended) {
      const { materializationsToRead, state } = response.suspended;
      const readOps = materializationRecordsToReadOps(materializationsToRead);
      const readResults = await this.readMaterializations(readOps);
      const materializations = readResultsToMaterializationRecords(readResults);

      // Resume with the fetched materializations
      const resumeResponse = this.resolver.resolveProcess({
        resume: { materializations, state },
      });

      if (!resumeResponse.resolved) {
        throw new Error('Resolve still suspended after providing materializations');
      }

      this.handleMaterializationWrites(resumeResponse.resolved.materializationsToWrite);
      return ResolveFlagsResponse.create(resumeResponse.resolved.response);
    }

    if (!response.resolved) {
      throw new Error('Unexpected empty resolve response');
    }

    this.handleMaterializationWrites(response.resolved.materializationsToWrite);
    return ResolveFlagsResponse.create(response.resolved.response);
  }

  private handleMaterializationWrites(
    records: { unit: string; materialization: string; rule: string; variant: string }[],
  ): void {
    if (records.length > 0) {
      const writeOps = materializationRecordsToWriteOps(records);
      this.writeMaterializations(writeOps);
    }
  }

  async updateState(signal?: AbortSignal): Promise<void> {
    const hashHex = await sha256Hex(this.options.flagClientSecret);
    const { encryptionKey } = this.options;
    const cdnPath = encryptionKey ? `${hashHex}.enc` : hashHex;
    const cdnUrl = `https://confidence-resolver-state-cdn.spotifycdn.com/${cdnPath}`;

    const headers = new Headers();
    if (this.stateEtag) {
      headers.set('If-None-Match', this.stateEtag);
    }
    const resp = await this.fetch(cdnUrl, { headers, signal });
    if (resp.status === 304) {
      return;
    }
    if (!resp.ok) {
      throw new Error(`Failed to fetch state: ${resp.status} ${resp.statusText}`);
    }
    this.stateEtag = resp.headers.get('etag');

    const bytes = new Uint8Array(await resp.arrayBuffer());
    const sdk = { id: SdkId.SDK_ID_JS_LOCAL_SERVER_PROVIDER, version: VERSION };

    try {
      await this.flush(signal);
    } catch {
      // best-effort: don't block state update if flush fails
    }

    const plaintext = encryptionKey ? await decryptAesGcm(bytes, hexToBytes(encryptionKey)) : bytes;
    const clientState = ClientResolverState.decode(plaintext);
    this.logDestinations = clientState.logDestinations;
    this.accountId = clientState.account;
    this.resolver.setResolverState(
      SetResolverStateRequest.create({
        state: clientState.state,
        accountId: clientState.account,
        sdk,
        enableApplyDedup: this.options.enableApplyDedup ?? false,
        disableExposureCollection: this.options.disableExposureCollection === true,
      }),
    );
  }

  // TODO should this return success/failure, or even throw?
  async flush(signal?: AbortSignal): Promise<void> {
    let writeFlagLogRequest = this.resolver.flushLogs();
    if (writeFlagLogRequest.length > 0) {
      const includeInit = this.initTelemetryState === 'pending';
      if (includeInit) {
        this.initTelemetryState = 'sending';
        writeFlagLogRequest = this.addProviderInitTelemetry(writeFlagLogRequest);
      }
      const drainedFlushSucceeded = this.flushSucceeded;
      const drainedFlushFailed = this.flushFailed;
      const drainedEventsPublished = this.eventsPublished;
      const drainedEventBatchesSucceeded = this.eventBatchesSucceeded;
      const drainedEventBatchesFailed = this.eventBatchesFailed;
      writeFlagLogRequest = this.addFlushDeliveryTelemetry(writeFlagLogRequest);
      try {
        await this.sendFlagLogs(writeFlagLogRequest, signal);
        this.flushSucceeded++;
        if (includeInit) {
          this.initTelemetryState = 'sent';
        }
      } catch (error) {
        this.flushFailed++;
        this.flushSucceeded += drainedFlushSucceeded;
        this.flushFailed += drainedFlushFailed;
        this.eventsPublished += drainedEventsPublished;
        this.eventBatchesSucceeded += drainedEventBatchesSucceeded;
        this.eventBatchesFailed += drainedEventBatchesFailed;
        if (includeInit) {
          this.initTelemetryState = 'pending';
        }
        throw error;
      }
    }
  }

  private async flushAssigned(): Promise<void> {
    const writeFlagLogRequest = this.resolver.flushAssigned();
    if (writeFlagLogRequest.length > 0) {
      await this.sendFlagLogs(writeFlagLogRequest);
    }
  }

  private async sendFlagLogs(encodedWriteFlagLogRequest: Uint8Array, signal = this.main.signal): Promise<void> {
    const destinations =
      this.logDestinations.length > 0 ? this.logDestinations : [LogDestination.LOG_DESTINATION_SPOTIFY_EDGE];

    for (let i = 0; i < destinations.length; i++) {
      const isLast = i === destinations.length - 1;
      try {
        const ok = await this.sendFlagLogsToDestination(encodedWriteFlagLogRequest, destinations[i], signal);
        if (ok) return;
        // Non-OK response — try fallback if available
        if (!isLast) {
          logger.warn('Primary flag log destination returned error, trying fallback');
          continue;
        }
      } catch (err) {
        if (!isLast) {
          logger.warn('Primary flag log destination failed, trying fallback', err);
          continue;
        }
        // Last destination failed with network error — preserve original behavior
        logger.warn('Failed to send flag logs', err);
        throw err;
      }
    }
  }

  private addFlushDeliveryTelemetry(encodedWriteFlagLogRequest: Uint8Array): Uint8Array {
    const hasFlush = this.flushSucceeded > 0 || this.flushFailed > 0;
    const hasEvents = this.eventsPublished > 0 || this.eventBatchesSucceeded > 0 || this.eventBatchesFailed > 0;
    if (!hasFlush && !hasEvents) {
      return encodedWriteFlagLogRequest;
    }
    const request = WriteFlagLogsRequest.decode(encodedWriteFlagLogRequest);
    if (!request.telemetryData) {
      request.telemetryData = {
        resolverVersion: '',
        providerInitRate: [],
        resolveRate: [],
        memoryBytes: 0,
      };
    }
    const td = request.telemetryData!;
    if (hasFlush) {
      td.flush = { succeeded: this.flushSucceeded, failed: this.flushFailed };
    }
    if (hasEvents) {
      td.events = {
        published: this.eventsPublished,
        batchesSucceeded: this.eventBatchesSucceeded,
        batchesFailed: this.eventBatchesFailed,
      };
    }
    this.flushSucceeded = 0;
    this.flushFailed = 0;
    this.eventsPublished = 0;
    this.eventBatchesSucceeded = 0;
    this.eventBatchesFailed = 0;
    return WriteFlagLogsRequest.encode(request).finish();
  }

  private addProviderInitTelemetry(encodedWriteFlagLogRequest: Uint8Array): Uint8Array {
    const request = WriteFlagLogsRequest.decode(encodedWriteFlagLogRequest);
    if (!request.telemetryData) {
      request.telemetryData = {
        resolverVersion: '',
        providerInitRate: [],
        resolveRate: [],
        memoryBytes: 0,
      };
    }
    request.telemetryData.sdk = {
      id: SdkId.SDK_ID_JS_LOCAL_SERVER_PROVIDER,
      version: VERSION,
    };
    request.telemetryData.providerInitRate.push({ count: 1, labels: this.initLabels });
    return WriteFlagLogsRequest.encode(request).finish();
  }

  /**
   * Send flag logs to a specific destination. Returns true on success (HTTP 2xx),
   * false on a non-OK HTTP response. Throws on network errors.
   */
  private async sendFlagLogsToDestination(
    encodedWriteFlagLogRequest: Uint8Array,
    destination: LogDestination,
    signal: AbortSignal,
  ): Promise<boolean> {
    if (destination === LogDestination.LOG_DESTINATION_CLOUDFLARE) {
      const batch = WriteFlagLogsRequest.decode(encodedWriteFlagLogRequest);
      const ingestRequest = IngestFlagLogsRequest.encode(
        IngestFlagLogsRequest.create({ accountId: this.accountId, batch }),
      ).finish();

      const response = await this.fetch(
        'https://epx-flags-logs.experimentation-platform.workers.dev/v1/flagLogs:ingest',
        {
          method: 'post',
          signal,
          headers: {
            'Content-Type': 'application/x-protobuf',
            Authorization: `ClientSecret ${this.options.flagClientSecret}`,
          },
          body: ingestRequest as Uint8Array<ArrayBuffer>,
        },
      );
      if (!response.ok) {
        logger.error(
          `Failed to write flag logs to Cloudflare: ${response.status} ${
            response.statusText
          } - ${await response.text()}`,
        );
        return false;
      }
      return true;
    }

    // Edge (default — covers UNSPECIFIED and SPOTIFY_EDGE)
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
      return false;
    }
    return true;
  }

  private async readMaterializations(
    readOps: MaterializationStore.ReadOp[],
  ): Promise<MaterializationStore.ReadResult[]> {
    const materializationStore = this.materializationStore;
    if (materializationStore && typeof materializationStore.readMaterializations === 'function') {
      return materializationStore.readMaterializations(readOps);
    }
    throw new Error('Read materialization not supported');
  }

  private writeMaterializations(writeOps: MaterializationStore.WriteOp[]): void {
    const materializationStore = this.materializationStore;
    if (materializationStore && typeof materializationStore.writeMaterializations === 'function') {
      materializationStore.writeMaterializations(writeOps).catch(e => {
        logger.warn('Failed to write materialization', e);
      });
      return;
    }
    throw new Error('Write materialization not supported');
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

  /**
   * Returns a Prometheus metrics snapshot from the WASM resolver.
   *
   * @experimental This API is subject to change.
   */
  getPrometheusMetrics(_request?: SnapshotConfig): string {
    return this.resolver.prometheusSnapshot('0');
  }

  /**
   * Applies a previously resolved flag, logging that it was used/exposed.
   * Call this when a flag value is actually rendered or used in the client.
   * @param resolveToken - Base64-encoded resolve token from the flag bundle
   * @param flagName - Name of the flag to apply
   */
  applyFlag(resolveToken: string, flagName: string): void {
    const request = {
      flags: [
        {
          flag: `flags/${flagName}`,
          applyTime: new Date(),
        },
      ],
      clientSecret: this.options.flagClientSecret,
      resolveToken: FlagBundle.decodeToken(resolveToken),
      sendTime: new Date(),
      sdk: {
        id: SdkId.SDK_ID_JS_LOCAL_SERVER_PROVIDER,
        version: VERSION,
      },
    };

    this.resolver.applyFlags(request);
  }
}

async function decryptAesGcm(data: Uint8Array, rawKey: Uint8Array): Promise<Uint8Array> {
  const NONCE_LEN = 12;
  if (data.length < NONCE_LEN) {
    throw new Error('Encrypted state too short (missing nonce)');
  }
  const iv = data.buffer.slice(data.byteOffset, data.byteOffset + NONCE_LEN) as ArrayBuffer;
  const ciphertext = data.buffer.slice(data.byteOffset + NONCE_LEN, data.byteOffset + data.byteLength) as ArrayBuffer;
  const key = await crypto.subtle.importKey('raw', rawKey.buffer as ArrayBuffer, 'AES-GCM', false, ['decrypt']);
  const plaintext = await crypto.subtle.decrypt({ name: 'AES-GCM', iv }, key, ciphertext);
  return new Uint8Array(plaintext);
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
      return ResolveReason.RESOLVE_REASON_TARGETING_KEY_ERROR;
    case 'ERROR':
      return ResolveReason.RESOLVE_REASON_ERROR;
    default:
      return ResolveReason.RESOLVE_REASON_UNSPECIFIED;
  }
}
