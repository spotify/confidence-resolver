import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  cannedDetails,
  cannedError,
  cannedResolvedStart,
  encryptModuleWrapper,
  moduleArtifactBytes,
  stageCannedResponses,
} from './test-support';
import { ConfidenceModuleProvider, DEFAULT_FLUSH_INTERVAL } from './provider';
import { DEFAULT_MODULE_UPDATE_INTERVAL } from './module-client';
import type { ConfidenceBundleProvider } from '../provider-contract';

const CLIENT_SECRET = 'provider-test-secret';
const ENCRYPTION_KEY = '00'.repeat(32);
const MODULE_BASE = 'http://module-cdn.test';
const INGESTION_BASE = 'http://ingestion.test';
const LOG_BODY = 'LOGSBODYLOGSBODY';

type Recorded = { url: string; init?: RequestInit };
type RoutingFetch = typeof fetch & { requests: Recorded[]; setModuleAvailable(v: boolean): void };

function routingFetch(): RoutingFetch {
  const requests: Recorded[] = [];
  let moduleAvailable = true;
  const impl = ((url: string, init?: RequestInit) => {
    requests.push({ url, init });
    if (url.startsWith(MODULE_BASE)) {
      if (!moduleAvailable) return Promise.resolve(new Response('not published', { status: 404 }));
      return Promise.resolve(
        new Response(encryptModuleWrapper(moduleArtifactBytes(CLIENT_SECRET), 'header.{}.signature', ENCRYPTION_KEY), {
          status: 200,
          headers: { etag: 'etag-1' },
        }),
      );
    }
    return Promise.resolve(new Response('', { status: 200 }));
  }) as RoutingFetch;
  impl.requests = requests;
  impl.setModuleAvailable = (v: boolean) => {
    moduleAvailable = v;
  };
  return impl;
}

function makeProvider(
  fetchImpl: RoutingFetch,
  options?: Partial<ConstructorParameters<typeof ConfidenceModuleProvider>[0]>,
) {
  return new ConfidenceModuleProvider({
    flagClientSecret: CLIENT_SECRET,
    encryptionKey: ENCRYPTION_KEY,
    moduleBaseUrl: MODULE_BASE,
    ingestionBaseUrl: INGESTION_BASE,
    fetch: fetchImpl,
    ...options,
  });
}

const providers: ConfidenceModuleProvider[] = [];

afterEach(async () => {
  for (const provider of providers.splice(0)) {
    await provider.onClose().catch(() => undefined);
  }
});

function track<T extends ConfidenceModuleProvider>(provider: T): T {
  providers.push(provider);
  return provider;
}

/** Stages canned responses into every pooled instance's memory. */
async function stageAll(
  provider: ConfidenceModuleProvider,
  start: Uint8Array,
  resume?: Uint8Array,
  details?: Uint8Array,
) {
  await vi.waitFor(() => {
    expect(provider['runtime'].isReady).toBe(true);
  });
  stageCannedResponses(provider['runtime'].pool!.memory, start, resume, details);
}

describe('ConfidenceModuleProvider', () => {
  it('defaults module and flush intervals', () => {
    const provider = track(makeProvider(routingFetch()));
    expect(provider['moduleUpdateInterval']).toBe(DEFAULT_MODULE_UPDATE_INTERVAL);
    expect(provider['flushInterval']).toBe(DEFAULT_FLUSH_INTERVAL);
  });

  it('rejects invalid intervals', () => {
    expect(
      () =>
        new ConfidenceModuleProvider({
          flagClientSecret: CLIENT_SECRET,
          encryptionKey: ENCRYPTION_KEY,
          moduleUpdateInterval: 500,
        }),
    ).toThrow('moduleUpdateInterval');
    expect(
      () =>
        new ConfidenceModuleProvider({
          flagClientSecret: CLIENT_SECRET,
          encryptionKey: ENCRYPTION_KEY,
          flushInterval: 0,
        }),
    ).toThrow('flushInterval');
  });

  it('initializes and reaches READY once the module is accepted', async () => {
    const fetchImpl = routingFetch();
    const provider = track(makeProvider(fetchImpl));
    await provider.initialize();
    expect(provider.status).toBe('READY');
    expect(fetchImpl.requests.some(r => r.url.startsWith(MODULE_BASE))).toBe(true);
  });

  it('reports not-ready defaults from evaluate before initialization', async () => {
    const provider = track(makeProvider(routingFetch()));
    const details = await provider.resolveBooleanEvaluation('test-flag.enabled', false, { targetingKey: 'user-1' });
    expect(details).toEqual({
      value: false,
      reason: 'ERROR',
      errorCode: 'PROVIDER_NOT_READY',
      errorMessage: 'Provider is not ready',
    });
  });

  it('satisfies the shared provider contract used by the React integrations', async () => {
    const provider = track(makeProvider(routingFetch()));
    const contract: ConfidenceBundleProvider = provider; // type-level check
    await provider.initialize();
    expect(contract.metadata.name).toBe('ConfidenceModuleProvider');
  });

  it('exposes the module Prometheus metrics snapshot', async () => {
    const provider = track(makeProvider(routingFetch()));
    await provider.initialize();
    await expect(provider.getPrometheusMetrics()).resolves.toBe('fake-metrics');
  });

  it('evaluates through the module and maps reasons', async () => {
    const provider = track(makeProvider(routingFetch()));
    await provider.initialize();
    await stageAll(provider, cannedResolvedStart, undefined, cannedDetails());

    const details = await provider.resolveBooleanEvaluation('test-flag.enabled', false, { targetingKey: 'user-1' });
    expect(details.value).toBe(true);
    expect(details.reason).toBe('TARGETING_MATCH');
    expect(details.variant).toBe('on');
    expect(details.errorCode).toBeUndefined();
  });

  it('echoes the default for FLAG_NOT_FOUND from the module', async () => {
    const provider = track(makeProvider(routingFetch()));
    await provider.initialize();
    await stageAll(
      provider,
      cannedResolvedStart,
      undefined,
      cannedDetails({ errorCode: 3, errorMessage: 'no such flag' }),
    );

    const details = await provider.resolveBooleanEvaluation('test-flag.enabled', false, { targetingKey: 'user-1' });
    expect(details.value).toBe(false);
    expect(details.reason).toBe('ERROR');
    expect(details.errorCode).toBe('FLAG_NOT_FOUND');
    expect(details.errorMessage).toBe('no such flag');
  });

  it('maps process errors to OpenFeature error codes', async () => {
    const provider = track(makeProvider(routingFetch()));
    await provider.initialize();
    await stageAll(provider, cannedError);

    const details = await provider.resolveBooleanEvaluation('test-flag.enabled', false, { targetingKey: 'user-1' });
    expect(details.value).toBe(false);
    expect(details.reason).toBe('ERROR');
    expect(details.errorCode).toBe('PROVIDER_FATAL');
  });

  it('resolves bundles without applying, then applies them on demand', async () => {
    const provider = track(makeProvider(routingFetch()));
    await provider.initialize();
    await stageAll(provider, cannedResolvedStart);

    const bundle = await provider.resolve({ targetingKey: 'user-1' }, ['test-flag']);
    expect(bundle.resolveToken).toBe('AQIDBA=='); // base64 of token [1,2,3,4]
    expect(bundle.flags['test-flag']?.variant).toBe('control');

    // Fire-and-forget apply through the provider, then verify it landed: the
    // same token applied again through the runtime now reads as duplicate (1).
    provider.applyFlag(bundle.resolveToken, 'test-flag');
    await vi.waitFor(async () => {
      const duplicate = await provider['runtime'].applyFlags({
        resolveToken: new Uint8Array([1, 2, 3, 4]),
        flags: [{ flag: 'flags/test-flag' }],
      });
      expect(duplicate).toBe(1);
    });
  });

  it('flushes buffered logs on close with the wrapper token', async () => {
    const fetchImpl = routingFetch();
    const provider = track(makeProvider(fetchImpl));
    await provider.initialize();
    await stageAll(provider, cannedResolvedStart, undefined, cannedDetails());
    await provider.resolveBooleanEvaluation('test-flag.enabled', false, { targetingKey: 'user-1' });

    await provider.onClose();
    providers.length = 0; // already closed

    const ingestion = fetchImpl.requests.find(r => r.url.startsWith(INGESTION_BASE));
    expect(ingestion).toBeDefined();
    const auth = ingestion!.init!.headers as Record<string, string>;
    expect(auth.Authorization).toBe(`Bearer header.{}.signature`);
    const text = await new Response(ingestion!.init!.body as BodyInit).text();
    // the wrapped payload embeds the canned body inline
    expect(text).toContain('LOGSBODYLOGSBODY');
  });

  it('recovers to READY via polling after a failed initialization', { timeout: 15_000 }, async () => {
    const fetchImpl = routingFetch();
    fetchImpl.setModuleAvailable(false);
    const provider = track(makeProvider(fetchImpl, { initializeTimeout: 100 }));

    await expect(provider.initialize()).rejects.toThrow('Timed out');
    expect(provider.status).toBe('ERROR');

    fetchImpl.setModuleAvailable(true);
    await vi.waitFor(() => expect(provider.status).toBe('READY'), { timeout: 5000 });
  });
});
