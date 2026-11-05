import {
  ApplyFlagsRequest,
  ResolveFlagsRequest,
  ResolveFlagsResponse,
} from '../proto/confidence/flags/resolver/v1/api';
import {
  EvaluateRequest,
  Materialization,
  Materialization_Record,
  ResolveProcessResponse,
  ResolutionDetails,
  WriteLogsPayload,
} from '../proto/confidence/flags/wasm/v1/api';
import {
  type MaterializationStore,
  materializationRecordsToReadOps,
  materializationRecordsToWriteOps,
  readResultsToMaterializationRecords,
} from '../materialization';
import { ModuleClient, type CurrentModule, type ModuleRefreshOutcome } from './module-client';
import { GuestInstancePool } from './instance-pool';
import type { GuestInstance, GuestLogPayload, HostFunctions } from './guest-instance';
import { unpackSlice } from './slice';

/** Default ingestion host (regionless); the base URL is the only configurable part. */
export const DEFAULT_INGESTION_BASE_URL = 'https://ingestion.confidence.dev';

export interface ModuleRuntimeOptions {
  clientSecret: string;
  /** Hex-encoded AES-256 key paired with the credential. */
  encryptionKey: string;
  /** Module CDN base URL. */
  moduleBaseUrl?: string;
  /** Ingestion base URL for log delivery. Default: the regionless ingestion host. */
  ingestionBaseUrl?: string;
  fetch?: typeof fetch;
  /** Sticky assignments and materialized segments; absent stores make the module report MATERIALIZATION_NOT_SUPPORTED. */
  materializationStore?: MaterializationStore;
  maxInstances?: number;
  warmInstances?: number;
  /** SDK identity stamped on requests (custom_id + version). */
  sdk: { customId: string; version: string };
  /** Capacity of each exposure/schema de-duplication seen-set. Default 100_000. */
  seenSetCapacity?: number;
  /** Log delivery tuning; retries are bounded and batches are dropped rather than retried forever. */
  logDelivery?: { maxAttempts?: number; baseDelayMs?: number };
}

export interface ResolveInput {
  /** Flag keys to resolve; empty for every flag available to the credential. */
  flags: string[];
  /** OpenFeature evaluation context (already targeting_key shaped). */
  evaluationContext: Record<string, unknown>;
  /** Record exposures as part of the resolve, or defer until `applyFlags`. */
  apply: boolean;
}

export interface ApplyFlagsInput {
  /** Token from the resolve this apply belongs to. */
  resolveToken: Uint8Array;
  flags: Array<{ flag: string; applyTime?: Date }>;
}

/** A resolve rejected by the module; `code` follows the gRPC status codes. */
export class ResolveProcessError extends Error {
  constructor(
    readonly code: number,
    message: string,
  ) {
    super(message);
  }
}

/** The runtime has no module loaded yet (or was closed). */
export class RuntimeNotReadyError extends Error {
  constructor() {
    super('No module loaded');
  }
}

export interface ResolveSession {
  /** The final response (a host-owned copy, valid after release). */
  readonly response: ResolveFlagsResponse;
  /** The client the request authenticated as (diagnostic). */
  readonly clientIndex: number;
  /** Materialization records to persist (persisted best-effort by the runtime already). */
  readonly materializationsToWrite: Materialization_Record[];
  /** Whether the module provides the `evaluate` export. */
  readonly canEvaluate: boolean;
  /**
   * Evaluate one flag key against a default using the module's evaluation
   * step. Valid until `release()`; may be called repeatedly.
   */
  evaluate(
    flagKey: string,
    defaultValue?: unknown,
    expectedSchema?: EvaluateRequest['expectedSchema'],
  ): ResolutionDetails;
  /** Free the response envelope and release the leased instance. Call exactly once. */
  release(): void;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** A bounded "seen" set: check-and-mark, evicting the oldest half at capacity. */
class BoundedSeenSet {
  private readonly seen = new Set<bigint>();

  constructor(private readonly capacity: number) {}

  /** Returns true when the fingerprint was already seen; marks it seen either way. */
  check(fingerprint: bigint): boolean {
    const wasSeen = this.seen.has(fingerprint);
    if (!wasSeen) {
      if (this.seen.size >= this.capacity) {
        const toEvict = Math.ceil(this.seen.size / 2);
        let evicted = 0;
        for (const key of this.seen) {
          this.seen.delete(key);
          if (++evicted >= toEvict) break;
        }
      }
      this.seen.add(fingerprint);
    }
    return wasSeen;
  }
}

/**
 * Delivers log batches to the ingestion API. Each batch captures the token at
 * enqueue time and keeps it for its whole life: batches never merge across
 * tokens, retries are bounded, and nothing ever blocks a resolve. The token is
 * a bearer credential and is never logged.
 */
class LogQueue {
  private token = '';
  private closed = false;
  private inFlight = 0;
  readonly stats = { sent: 0, dropped: 0, retries: 0 };

  constructor(
    private readonly baseUrl: string,
    private readonly fetchImpl: typeof fetch,
    private readonly maxAttempts: number,
    private readonly baseDelayMs: number,
  ) {}

  setToken(token: string): void {
    this.token = token;
  }

  get closedState(): boolean {
    return this.closed;
  }

  enqueue(body: Uint8Array<ArrayBuffer>): void {
    if (this.closed) {
      this.stats.dropped++;
      return;
    }
    void this.deliver(body, this.token, 0);
  }

  /** Wait until no batch is mid-delivery (bounded); used by shutdown drains. */
  async settle(timeoutMs: number): Promise<void> {
    const deadline = Date.now() + timeoutMs;
    while (this.inFlight > 0 && Date.now() < deadline) {
      await new Promise(resolve => setTimeout(resolve, 5));
    }
  }

  close(): void {
    this.closed = true;
  }

  private async deliver(body: Uint8Array<ArrayBuffer>, token: string, attempt: number): Promise<void> {
    this.inFlight++;
    try {
      await this.deliverAttempt(body, token, attempt);
    } finally {
      this.inFlight--;
    }
  }

  private async deliverAttempt(body: Uint8Array<ArrayBuffer>, token: string, attempt: number): Promise<void> {
    if (this.closed) {
      this.stats.dropped++;
      return;
    }
    let retryable = false;
    try {
      const response = await this.fetchImpl(`${this.baseUrl}/v1/flagLogs:write`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/x-protobuf', Authorization: `Bearer ${token}` },
        body,
      });
      if (response.ok) {
        this.stats.sent++;
        return;
      }
      retryable = response.status === 408 || response.status === 429 || response.status >= 500;
    } catch {
      retryable = true;
    }
    if (!retryable || attempt + 1 >= this.maxAttempts) {
      this.stats.dropped++;
      return;
    }
    this.stats.retries++;
    setTimeout(() => void this.deliver(body, token, attempt + 1), this.baseDelayMs * 2 ** attempt);
  }
}

/**
 * Runs one credential's compiled client module: fetches and verifies the
 * artifact (through a {@link ModuleClient}), pools instances over the shared
 * memory, drives the resolve/suspend/resume protocol, applies previously
 * resolved flags, and forwards the module's logs to the ingestion API with
 * the wrapper's token.
 *
 * A failed refresh never takes a running module down; only a successfully
 * verified replacement switches the pool.
 */
export class ModuleRuntime {
  private poolField: GuestInstancePool | null = null;
  private readonly logQueue: LogQueue;
  private readonly seenApply: BoundedSeenSet;
  private readonly seenSchema: BoundedSeenSet;
  private readonly hostFunctions: HostFunctions;

  constructor(
    private readonly moduleClient: ModuleClient,
    private readonly options: ModuleRuntimeOptions,
  ) {
    this.logQueue = new LogQueue(
      (options.ingestionBaseUrl ?? DEFAULT_INGESTION_BASE_URL).replace(/\/+$/, ''),
      options.fetch ?? fetch,
      options.logDelivery?.maxAttempts ?? 3,
      options.logDelivery?.baseDelayMs ?? 500,
    );
    const capacity = options.seenSetCapacity ?? 100_000;
    this.seenApply = new BoundedSeenSet(capacity);
    this.seenSchema = new BoundedSeenSet(capacity);
    this.hostFunctions = {
      currentTime: () => BigInt(Date.now()),
      randomSeed: () => {
        const bytes = new Uint8Array(8);
        crypto.getRandomValues(bytes);
        return new DataView(bytes.buffer).getBigUint64(0, true);
      },
      writeLogs: payload => this.handleWriteLogs(payload),
      applyFilterSeen: fingerprint => this.seenApply.check(fingerprint),
      schemaFilterSeen: fingerprint => this.seenSchema.check(fingerprint),
    };
  }

  /** The currently accepted artifact, or null before the first accept. */
  get current(): CurrentModule | null {
    return this.moduleClient.current;
  }

  /** The live instance pool, or null while no module is loaded. */
  get pool(): GuestInstancePool | null {
    return this.poolField;
  }

  get isReady(): boolean {
    return this.poolField !== null;
  }

  /** Log delivery counters: batches sent, dropped, and retried. */
  get logStats(): { sent: number; dropped: number; retries: number } {
    return this.logQueue.stats;
  }

  /**
   * One artifact refresh cycle plus pool choreography: a wrapper-only change
   * swaps the log token; a new module builds a new pool, switches routing to
   * it, then drains and destroys the old one. A pool-build failure keeps the
   * previous pool serving.
   */
  async refresh(signal?: AbortSignal): Promise<ModuleRefreshOutcome> {
    const outcome = await this.moduleClient.refresh(signal);
    if (outcome.type !== 'accepted') return outcome;

    if (outcome.change === 'token') {
      this.logQueue.setToken(outcome.current.logToken);
      return outcome;
    }
    let newPool: GuestInstancePool;
    try {
      newPool = await GuestInstancePool.create(outcome.current.moduleBytes, this.hostFunctions, {
        maxInstances: this.options.maxInstances,
        warmInstances: this.options.warmInstances,
      });
    } catch (error) {
      return { type: 'failed', reason: `failed to build instance pool: ${errorMessage(error)}` };
    }
    this.logQueue.setToken(outcome.current.logToken);
    const previousPool = this.poolField;
    this.poolField = newPool;
    if (previousPool) {
      // Stop routing happened above; drain in-flight holders, then destroy.
      await previousPool.destroy(signal).catch(() => undefined);
    }
    return outcome;
  }

  /**
   * Resolve flags, driving the suspend/resume protocol through the
   * materialization store. The returned session holds the leased instance and
   * the borrowed response envelope: call `release()` exactly once when done
   * evaluating.
   */
  async resolve(input: ResolveInput): Promise<ResolveSession> {
    const pool = this.poolField;
    if (!pool) throw new RuntimeNotReadyError();
    const instance = await pool.acquire();
    const startMs = performance.now();
    try {
      const request = ResolveFlagsRequest.encode({
        flags: input.flags,
        evaluationContext: input.evaluationContext,
        clientSecret: this.options.clientSecret,
        apply: input.apply,
        sdk: { customId: this.options.sdk.customId, version: this.options.sdk.version },
      }).finish();

      let envelope = instance.resolveProcessStart(request);
      let decoded = ResolveProcessResponse.decode(instance.readSlice(envelope));

      if (decoded.suspended) {
        const { processId, materializationToRead } = decoded.suspended;
        let records: Materialization_Record[] = [];
        try {
          if (this.options.materializationStore) {
            records = await this.readMaterializations(materializationToRead);
          }
        } catch (error) {
          instance.resolveProcessDiscard(processId);
          instance.free(unpackSlice(envelope).ptr);
          throw error;
        }
        const resumed = instance.resolveProcessResume(processId, Materialization.encode({ records }).finish());
        instance.free(unpackSlice(envelope).ptr); // the suspended envelope is spent
        envelope = resumed;
        decoded = ResolveProcessResponse.decode(instance.readSlice(envelope));
        if (decoded.suspended) {
          // A second suspension is a module defect: discard and fail the resolve.
          instance.resolveProcessDiscard(decoded.suspended.processId);
          instance.free(unpackSlice(envelope).ptr);
          throw new Error('Resolve suspended again after providing materializations');
        }
      }

      if (decoded.error) {
        instance.free(unpackSlice(envelope).ptr);
        throw new ResolveProcessError(decoded.error.code, decoded.error.message);
      }
      const resolved = decoded.resolved;
      if (!resolved?.response) {
        instance.free(unpackSlice(envelope).ptr);
        throw new Error('Resolve returned an empty response');
      }

      if (instance.capabilities.observeClientResolveLatency) {
        const micros = Math.round((performance.now() - startMs) * 1000);
        try {
          instance.observeClientResolveLatency(resolved.clientIndex, BigInt(micros));
        } catch {
          // best-effort telemetry
        }
      }
      this.persistMaterializations(resolved.materializationToWrite ?? []);

      return {
        response: resolved.response,
        clientIndex: resolved.clientIndex,
        materializationsToWrite: resolved.materializationToWrite ?? [],
        canEvaluate: instance.capabilities.evaluate,
        evaluate: (flagKey, defaultValue, expectedSchema) =>
          this.evaluateOn(instance, envelope, flagKey, defaultValue, expectedSchema),
        release: () => {
          instance.free(unpackSlice(envelope).ptr);
          pool.release(instance);
        },
      };
    } catch (error) {
      pool.release(instance);
      throw error;
    }
  }

  /** Apply previously resolved flags (best-effort logging; returns the module status code). */
  async applyFlags(input: ApplyFlagsInput): Promise<number> {
    const pool = this.poolField;
    if (!pool) throw new RuntimeNotReadyError();
    const instance = await pool.acquire();
    try {
      const request = ApplyFlagsRequest.encode({
        clientSecret: this.options.clientSecret,
        resolveToken: input.resolveToken,
        sendTime: new Date(),
        flags: input.flags.map(flag => ({ flag: flag.flag, applyTime: flag.applyTime ?? new Date() })),
        sdk: { customId: this.options.sdk.customId, version: this.options.sdk.version },
      }).finish();
      return instance.applyFlags(request);
    } finally {
      pool.release(instance);
    }
  }

  /** Flush buffered logs now, on an idle instance. Skips when every instance is busy. */
  flushLogs(): void {
    this.poolField?.tryFlushLogsIdle();
  }

  /** Flush now and wait (bounded) for in-flight deliveries to finish; for shutdown drains. */
  async drainLogs(timeoutMs = 5000): Promise<void> {
    this.flushLogs();
    await this.logQueue.settle(timeoutMs);
  }

  /** Module-wide metrics snapshot in text form (Prometheus or OpenMetrics). */
  async metricsSnapshot(format: 0 | 1 = 0): Promise<string> {
    const pool = this.poolField;
    if (!pool) throw new RuntimeNotReadyError();
    const instance = await pool.acquire();
    try {
      return new TextDecoder().decode(instance.metricsSnapshot(format));
    } finally {
      pool.release(instance);
    }
  }

  /** Bytes currently allocated by the module. */
  async allocatedBytes(): Promise<number> {
    const pool = this.poolField;
    if (!pool) throw new RuntimeNotReadyError();
    const instance = await pool.acquire();
    try {
      return instance.allocatedBytes();
    } finally {
      pool.release(instance);
    }
  }

  /** Destroy the pool and stop log delivery. Pending batches are dropped. */
  async close(signal?: AbortSignal): Promise<void> {
    const pool = this.poolField;
    this.poolField = null;
    this.logQueue.close();
    await pool?.destroy(signal).catch(() => undefined);
  }

  private evaluateOn(
    instance: GuestInstance,
    envelope: ReturnType<GuestInstance['resolveProcessStart']>,
    flagKey: string,
    defaultValue: unknown,
    expectedSchema: EvaluateRequest['expectedSchema'],
  ): ResolutionDetails {
    const request = EvaluateRequest.encode({ flagKey, defaultValue, expectedSchema }).finish();
    return ResolutionDetails.decode(instance.evaluate(envelope, request));
  }

  private handleWriteLogs(payload: GuestLogPayload): void {
    // Parts are callback-scoped slices into module memory: copy them out and
    // concatenate, in order, into one owned buffer. ts-proto decodes fixed64
    // as number (exact below 2^53, far above any real slice).
    const envelope = WriteLogsPayload.decode(payload.bytes());
    const chunks: Uint8Array[] = [];
    for (const part of envelope.parts) {
      const packed = BigInt(part);
      if (unpackSlice(packed).len === 0) continue;
      chunks.push(payload.readSlice(packed));
    }
    const body = new Uint8Array(chunks.reduce((sum, chunk) => sum + chunk.length, 0));
    let offset = 0;
    for (const chunk of chunks) {
      body.set(chunk, offset);
      offset += chunk.length;
    }
    // In client modules the payload token is empty: the wrapper's token (held
    // by the queue) is the credential. Hand the body to the queue and return;
    // the payload is acked by the GuestInstance wrapper.
    this.logQueue.enqueue(body);
  }

  private async readMaterializations(records: Materialization_Record[]): Promise<Materialization_Record[]> {
    const store = this.options.materializationStore;
    if (!store) return [];
    const results = await store.readMaterializations(materializationRecordsToReadOps(records));
    return readResultsToMaterializationRecords(results);
  }

  /** Best-effort persistence: a slow or failing store must never fail anything. */
  private persistMaterializations(records: Materialization_Record[]): void {
    const store = this.options.materializationStore;
    if (!store?.writeMaterializations || records.length === 0) return;
    store.writeMaterializations(materializationRecordsToWriteOps(records)).catch(() => undefined);
  }
}
