import { describe, expect, it, vi } from 'vitest';
import type { MaterializationStore } from './materialization';
import { ResolveReason } from './proto/confidence/flags/resolver/v1/types';
import { ModuleClient } from './module-client';
import { LocalResolver, ResolveProcessError, RuntimeNotReadyError } from './LocalResolver';
import {
  cannedDetails,
  cannedError,
  cannedResolvedAfterResume,
  cannedResolvedStart,
  cannedSuspended,
  encryptModuleWrapper,
  moduleArtifactBytes,
  stageCannedResponses,
} from './test-helpers';

const CLIENT_SECRET = 'resolver-test-secret';
const ENCRYPTION_KEY = '00'.repeat(32);
const MODULE_BASE = 'http://module-cdn.test';
const INGESTION_BASE = 'http://ingestion.test';
const LOG_BODY = 'LOGSBODYLOGSBODY';

// --- fixtures -------------------------------------------------------------------

const moduleArtifact = (variant = 1) => moduleArtifactBytes(CLIENT_SECRET, variant);

const encryptedModuleResponse = (moduleBytes: Uint8Array, logToken: string, etag: string) =>
  new Response(encryptModuleWrapper(moduleBytes, logToken, ENCRYPTION_KEY), { status: 200, headers: { etag } });

type RecordingFetch = typeof fetch & { requests: Array<{ url: string; init?: RequestInit }> };

function recordingFetch(handler: (call: number, url: string, init?: RequestInit) => Response): RecordingFetch {
  const requests: RecordingFetch['requests'] = [];
  let call = 0;
  const impl = ((url: string, init?: RequestInit) => {
    requests.push({ url, init });
    return Promise.resolve(handler(call++, url, init));
  }) as RecordingFetch;
  impl.requests = requests;
  return impl;
}

const resolvedStart = cannedResolvedStart;
const suspendedResponse = cannedSuspended;
const resolvedAfterResume = cannedResolvedAfterResume;
const errorResponse = cannedError;

// --- harness --------------------------------------------------------------------

const stage = stageCannedResponses;

function discardCount(memory: WebAssembly.Memory): number {
  return Atomics.load(new Int32Array(memory.buffer), 11);
}

function createRecordingStore(): MaterializationStore & {
  readMaterializations: ReturnType<typeof vi.fn>;
  writeMaterializations: ReturnType<typeof vi.fn>;
} {
  return {
    readMaterializations: vi.fn(async (): Promise<MaterializationStore.ReadResult[]> => [
      { op: 'variant', unit: 'user-1', materialization: 'm1', rule: 'r1', variant: 'sticky-1' },
    ]),
    writeMaterializations: vi.fn(async () => undefined),
  };
}

async function createResolver(overrides?: {
  moduleHandler?: (call: number, url: string, init?: RequestInit) => Response;
  ingestionHandler?: (call: number) => Response;
  store?: MaterializationStore;
  logDelivery?: { maxBatchBytes?: number; timeoutMs?: number };
}) {
  const moduleFetch = recordingFetch(
    overrides?.moduleHandler ?? (() => encryptedModuleResponse(moduleArtifact(), 'token-1', '"v1"')),
  );
  const ingestionFetch = recordingFetch(overrides?.ingestionHandler ?? (() => new Response(null, { status: 200 })));
  const resolver = new LocalResolver(
    new ModuleClient({
      clientSecret: CLIENT_SECRET,
      encryptionKey: ENCRYPTION_KEY,
      baseUrl: MODULE_BASE,
      fetch: moduleFetch,
    }),
    {
      clientSecret: CLIENT_SECRET,
      encryptionKey: ENCRYPTION_KEY,
      ingestionBaseUrl: INGESTION_BASE,
      fetch: ingestionFetch,
      materializationStore: overrides?.store,
      maxInstances: 4,
      warmInstances: 1,
      sdk: { customId: 'test-resolver', version: '1.0.0' },
      logDelivery: { maxAttempts: 2, baseDelayMs: 5, ...overrides?.logDelivery },
    },
  );
  return { resolver, moduleFetch, ingestionFetch };
}

async function readyResolver(overrides?: Parameters<typeof createResolver>[0]) {
  const harness = await createResolver(overrides);
  await harness.resolver.updateState();
  return harness;
}

const flushMicrotasks = () => new Promise(resolve => setImmediate(resolve));
const sleep = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));

// --- tests ----------------------------------------------------------------------

describe('LocalResolver refresh', () => {
  it('builds the pool on the first accept', async () => {
    const { resolver } = await createResolver();
    expect(resolver.isReady).toBe(false);
    const outcome = await resolver.updateState();
    expect(outcome).toMatchObject({ type: 'accepted', change: 'initial' });
    expect(resolver.isReady).toBe(true);
    expect(resolver.pool!.liveCount).toBe(1);
    expect(resolver.current!.logToken).toBe('token-1');
    await resolver.close();
  });

  it('swaps only the log token on wrapper-only changes', async () => {
    const { resolver, ingestionFetch } = await readyResolver({
      moduleHandler: call =>
        encryptedModuleResponse(moduleArtifact(), call === 0 ? 'token-1' : 'token-2', call === 0 ? '"v1"' : '"v2"'),
    });

    stage(resolver.pool!.memory, resolvedStart);
    resolver.flushLogs();
    await flushMicrotasks();
    expect(ingestionFetch.requests).toHaveLength(1);
    expect((ingestionFetch.requests[0].init!.headers as Record<string, string>).Authorization).toBe('Bearer token-1');

    const outcome = await resolver.updateState();
    expect(outcome).toMatchObject({ type: 'accepted', change: 'token' });
    const firstPool = resolver.pool;

    resolver.flushLogs();
    await flushMicrotasks();
    expect(ingestionFetch.requests).toHaveLength(2);
    expect((ingestionFetch.requests[1].init!.headers as Record<string, string>).Authorization).toBe('Bearer token-2');
    expect(resolver.pool).toBe(firstPool); // nothing recompiled
    await resolver.close();
  });

  it('rebuilds the pool on module changes and keeps the old one drained', async () => {
    const { resolver } = await readyResolver({
      moduleHandler: call =>
        encryptedModuleResponse(moduleArtifact(call === 0 ? 1 : 2), 'token-1', call === 0 ? '"v1"' : '"v2"'),
    });
    const oldPool = resolver.pool!;

    const outcome = await resolver.updateState();
    expect(outcome).toMatchObject({ type: 'accepted', change: 'module' });
    expect(resolver.pool).not.toBe(oldPool);
    expect(oldPool.liveCount).toBe(0); // drained and destroyed
    expect(resolver.isReady).toBe(true);

    // the new pool serves resolves
    stage(resolver.pool!.memory, resolvedStart, undefined, cannedDetails());
    const session = await resolver.resolve({
      flags: ['test-flag'],
      evaluationContext: { targeting_key: 'u1' },
      apply: true,
    });
    expect(session.response.resolveId).toBe('r1');
    session.release();
    await resolver.close();
  });
});

describe('LocalResolver resolve', () => {
  it('resolves, evaluates against the borrowed envelope, and releases', async () => {
    const { resolver } = await readyResolver();
    stage(resolver.pool!.memory, resolvedStart, resolvedAfterResume, cannedDetails());

    const session = await resolver.resolve({
      flags: ['test-flag'],
      evaluationContext: { targeting_key: 'user-1', country: 'SE' },
      apply: true,
    });
    expect(session.response.resolveId).toBe('r1');
    expect(session.response.resolvedFlags[0].flag).toBe('flags/test-flag');
    expect(session.response.resolvedFlags[0].value).toEqual({ fields: { enabled: true } });
    expect(session.clientIndex).toBe(7);
    expect(session.materializationsToWrite).toEqual([]);
    expect(session.canEvaluate).toBe(true);

    const details = session.evaluate('test-flag.enabled', false);
    expect(details.value).toBe(true);
    expect(details.variant).toBe('on');
    expect(details.reason).toBe(ResolveReason.RESOLVE_REASON_MATCH);
    expect(details.shouldApply).toBe(true);

    session.release();
    expect(resolver.pool!.heldCount).toBe(0);
    await resolver.close();
  });

  it('drives the suspend/resume protocol through the materialization store', async () => {
    const store = createRecordingStore();
    const { resolver } = await readyResolver({ store });
    stage(resolver.pool!.memory, suspendedResponse, resolvedAfterResume, cannedDetails());

    const session = await resolver.resolve({
      flags: ['test-flag'],
      evaluationContext: { targeting_key: 'user-1' },
      apply: true,
    });
    expect(store.readMaterializations).toHaveBeenCalledWith([
      { op: 'variant', unit: 'user-1', materialization: 'm1', rule: 'r1' },
    ]);
    expect(session.response.resolveId).toBe('r2');
    expect(session.response.resolvedFlags[0].variant).toBe('sticky');
    expect(session.materializationsToWrite).toEqual([
      { unit: 'user-1', materialization: 'm1', rule: 'r1', variant: 'sticky-1' },
    ]);
    session.release();

    await flushMicrotasks(); // fire-and-forget persistence
    expect(store.writeMaterializations).toHaveBeenCalledWith([
      { op: 'variant', unit: 'user-1', materialization: 'm1', rule: 'r1', variant: 'sticky-1' },
    ]);
    await resolver.close();
  });

  it('discards and fails the resolve when the store read fails', async () => {
    const store = createRecordingStore();
    store.readMaterializations.mockRejectedValueOnce(new Error('store down'));
    const { resolver } = await readyResolver({ store });
    stage(resolver.pool!.memory, suspendedResponse, resolvedAfterResume, cannedDetails());

    await expect(
      resolver.resolve({ flags: ['test-flag'], evaluationContext: { targeting_key: 'user-1' }, apply: true }),
    ).rejects.toThrow('store down');
    expect(discardCount(resolver.pool!.memory)).toBe(1);
    expect(resolver.pool!.heldCount).toBe(0);
    await resolver.close();
  });

  it('resumes with empty materializations when no store is configured', async () => {
    const { resolver } = await readyResolver();
    stage(resolver.pool!.memory, suspendedResponse, resolvedAfterResume, cannedDetails());
    const session = await resolver.resolve({
      flags: ['test-flag'],
      evaluationContext: { targeting_key: 'user-1' },
      apply: true,
    });
    expect(session.response.resolveId).toBe('r2');
    session.release();
    await resolver.close();
  });

  it('surfaces module rejections as ResolveProcessError and releases the lease', async () => {
    const { resolver } = await readyResolver();
    stage(resolver.pool!.memory, errorResponse);

    const promise = resolver.resolve({ flags: ['test-flag'], evaluationContext: {}, apply: true });
    await expect(promise).rejects.toBeInstanceOf(ResolveProcessError);
    const rejection = await promise.catch(error => error);
    expect(rejection.code).toBe(16);
    expect(rejection.message).toBe('invalid credential');
    expect(resolver.pool!.heldCount).toBe(0);
    await resolver.close();
  });

  it('rejects when no module is loaded', async () => {
    const { resolver } = await createResolver();
    await expect(resolver.resolve({ flags: [], evaluationContext: {}, apply: true })).rejects.toBeInstanceOf(
      RuntimeNotReadyError,
    );
    await expect(
      resolver.applyFlags({ resolveToken: new Uint8Array(0), flags: [{ flag: 'x' }] }),
    ).rejects.toBeInstanceOf(RuntimeNotReadyError);
    expect(() => resolver.flushLogs()).not.toThrow(); // no-op without a pool
  });
});

describe('LocalResolver apply', () => {
  it('applies flags and consults the exposure dedup filter', async () => {
    const { resolver } = await readyResolver();
    const input = { resolveToken: new Uint8Array([1, 2, 3, 4]), flags: [{ flag: 'flags/test-flag' }] };

    expect(await resolver.applyFlags(input)).toBe(0);
    // The canned guest fingerprints the request's stable prefix (client secret):
    // the second apply of the same credential is deduplicated.
    expect(await resolver.applyFlags(input)).toBe(1);
    await resolver.close();
  });
});

describe('LocalResolver log delivery', () => {
  it('forwards write_logs batches unchanged with the wrapper token', async () => {
    const { resolver, ingestionFetch } = await readyResolver();
    stage(resolver.pool!.memory, resolvedStart);

    resolver.flushLogs();
    await flushMicrotasks();

    expect(ingestionFetch.requests).toHaveLength(1);
    const request = ingestionFetch.requests[0];
    expect(request.url).toBe(`${INGESTION_BASE}/v1/flagLogs:write`);
    expect(request.init!.method).toBe('POST');
    const headers = request.init!.headers as Record<string, string>;
    expect(headers['Content-Type']).toBe('application/x-protobuf');
    expect(headers.Authorization).toBe('Bearer token-1');
    expect(new TextDecoder().decode(request.init!.body as Uint8Array)).toBe(LOG_BODY);
    expect(resolver.logStats.sent).toBe(1);
    await resolver.close();
  });

  it('ignores the deprecated payload log_token: the wrapper token always wins', async () => {
    const { resolver, ingestionFetch } = await readyResolver();
    stage(resolver.pool!.memory, resolvedStart);
    new DataView(resolver.pool!.memory.buffer).setUint8(35, 1); // module embeds a log_token

    resolver.flushLogs();
    await flushMicrotasks();

    expect(ingestionFetch.requests).toHaveLength(1);
    // The wrapper's token authenticates even when the module ships one: the
    // compiled-module token is deprecated and deliberately ignored.
    expect((ingestionFetch.requests[0].init!.headers as Record<string, string>).Authorization).toBe('Bearer token-1');
    await resolver.close();
  });

  it('drops batches over maxBatchBytes instead of retrying a doomed request', async () => {
    const { resolver, ingestionFetch } = await readyResolver({ logDelivery: { maxBatchBytes: 8 } });
    stage(resolver.pool!.memory, resolvedStart);

    resolver.flushLogs();
    await flushMicrotasks();

    expect(ingestionFetch.requests).toHaveLength(0);
    expect(resolver.logStats.dropped).toBe(1);
    await resolver.close();
  });

  it('retries retryable failures with a bounded budget, then succeeds', async () => {
    const { resolver } = await readyResolver({
      ingestionHandler: call => new Response(null, { status: call === 0 ? 500 : 200 }),
    });
    stage(resolver.pool!.memory, resolvedStart);

    resolver.flushLogs();
    await sleep(30);
    expect(resolver.logStats).toEqual({ sent: 1, dropped: 0, retries: 1 });
    await resolver.close();
  });

  it('drops batches after exhausting the retry budget', async () => {
    const { resolver } = await readyResolver({
      ingestionHandler: () => new Response(null, { status: 500 }),
    });
    stage(resolver.pool!.memory, resolvedStart);

    resolver.flushLogs();
    await sleep(30);
    expect(resolver.logStats).toEqual({ sent: 0, dropped: 1, retries: 1 });
    await resolver.close();
  });
});

describe('LocalResolver close', () => {
  it('destroys the pool and stops accepting work', async () => {
    const { resolver } = await readyResolver();
    stage(resolver.pool!.memory, resolvedStart, undefined, cannedDetails());
    await resolver.close();

    expect(resolver.isReady).toBe(false);
    await expect(resolver.resolve({ flags: [], evaluationContext: {}, apply: true })).rejects.toBeInstanceOf(
      RuntimeNotReadyError,
    );
    expect(() => resolver.flushLogs()).not.toThrow();
  });
});
