import { useFakeTimerCompatibleCrypto } from './test-helpers';
import { encryptTestState } from './test-helpers';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, MockedObject, test, vi } from 'vitest';
import { LocalResolver } from './LocalResolver';
import {
  ConfidenceServerProviderLocal,
  DEFAULT_FLUSH_INTERVAL,
  DEFAULT_INITIALIZE_TIMEOUT,
  DEFAULT_STATE_INTERVAL,
  NOT_READY_STATE_INTERVAL,
} from './ConfidenceServerProviderLocal';
import { abortableSleep, TimeUnit, timeoutSignal } from './util';
import { advanceTimersUntil, NetworkMock, noopEventTracker } from './test-helpers';
import { sha256Hex } from './hash';
import { ResolveReason } from './proto/confidence/flags/resolver/v1/types';
import { WriteFlagLogsRequest } from './proto/test-only';
import { ClientResolverState, LogDestination } from './proto/confidence/flags/admin/v1/resolver';
import { VERSION } from './version';
import { OpenFeature, ProviderStatus, ProviderEvents } from '@openfeature/server-sdk';
// Type-only: pins the README's documented entry point without loading its WASM.
import type * as NodeEntry from './index.node';

vi.mock(import('./hash'), async () => {
  const { sha256Hex } = await import('./test-helpers');
  return {
    sha256Hex,
  };
});

const mockedWasmResolver: MockedObject<LocalResolver> = {
  resolveProcess: vi.fn(),
  registerResolve: vi.fn(),
  setResolverState: vi.fn(),
  flushLogs: vi.fn().mockReturnValue(new Uint8Array(100)),
  flushAssigned: vi.fn().mockReturnValue(new Uint8Array(50)),
  applyFlags: vi.fn(),
  prometheusSnapshot: vi.fn().mockReturnValue(''),
};

let provider: ConfidenceServerProviderLocal;
let net: NetworkMock;

useFakeTimerCompatibleCrypto();
vi.useFakeTimers();

beforeEach(() => {
  vi.clearAllMocks();
  vi.clearAllTimers();
  vi.setSystemTime(0);
  net = new NetworkMock();
  provider = new ConfidenceServerProviderLocal(mockedWasmResolver, noopEventTracker, {
    flagClientSecret: 'flagClientSecret',
    encryptionKey: '00'.repeat(32),
    fetch: net.fetch,
    materializationStore: 'CONFIDENCE_REMOTE_STORE',
  });
});

afterEach(() => {});

describe('idealized conditions', () => {
  it('makes some requests', { timeout: 30_000 }, async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    const stateCallsAfterInit = net.cdn.state.calls;
    const flushCallsAfterInit = net.resolver.flagLogs.calls;

    // Let real WebCrypto finish each update before advancing the next interval.
    for (let i = 0; i < 120; i++) {
      const updated = new Promise<void>(resolve => {
        mockedWasmResolver.setResolverState.mockImplementationOnce(() => resolve());
      });
      await vi.advanceTimersByTimeAsync(DEFAULT_STATE_INTERVAL);
      await updated;
    }
    await vi.advanceTimersByTimeAsync(TimeUnit.SECOND);

    // since we fetch state every 30s we should fetch 120 times after init
    expect(net.cdn.state.calls).toBe(stateCallsAfterInit + 120);
    // flush is called every 15s (240) plus once before each state update (120)
    expect(net.resolver.flagLogs.calls).toBe(flushCallsAfterInit + 240 + 120);

    const flushCallsBeforeClose = net.resolver.flagLogs.calls;
    await advanceTimersUntil(expect(provider.onClose()).resolves.toBeUndefined());

    // close does a final flush
    expect(net.resolver.flagLogs.calls).toBe(flushCallsBeforeClose + 1);
  });
});

describe('no network', () => {
  beforeEach(() => {
    net.error = 'No network';
  });

  it('starts in NOT_READY and keeps retrying after the initialization timeout', async () => {
    await advanceTimersUntil(expect(provider.initialize()).rejects.toMatchObject({ code: 'PROVIDER_NOT_READY' }));

    expect(provider.status).toBe('ERROR');
    expect(Date.now()).toBe(DEFAULT_STATE_INTERVAL);

    const callsAfterInit = net.calls;
    await vi.advanceTimersByTimeAsync(NOT_READY_STATE_INTERVAL * 3);

    expect(net.calls).toBeGreaterThan(callsAfterInit);
  });
});

describe('state update scheduling', () => {
  it('fetches resolverStateUri on initialize', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());
    expect(net.cdn.state.calls).toBe(1);
  });
  it('polls state at fixed interval', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());
    // Initialize should trigger at least 1 state fetch
    expect(net.cdn.state.calls).toBeGreaterThanOrEqual(1);
    const callsAfterInit = net.cdn.state.calls;

    await vi.advanceTimersByTimeAsync(DEFAULT_STATE_INTERVAL);
    expect(net.cdn.state.calls).toBe(callsAfterInit + 1);

    await vi.advanceTimersByTimeAsync(DEFAULT_STATE_INTERVAL);
    expect(net.cdn.state.calls).toBe(callsAfterInit + 2);
  });
  it('honors If-None-Match and handles 304 Not Modified', async () => {
    let eTag = 'v1';
    const payload = new Uint8Array(100);
    net.cdn.state.handler = req => {
      const ifNoneMatch = req.headers.get('If-None-Match');
      if (ifNoneMatch === eTag) {
        return new Response(null, { status: 304 });
      }
      return new Response(encryptTestState(payload), { headers: { eTag } });
    };

    await advanceTimersUntil(provider.updateState());
    expect(mockedWasmResolver.setResolverState).toHaveBeenCalledTimes(1);

    await advanceTimersUntil(provider.updateState());
    expect(mockedWasmResolver.setResolverState).toHaveBeenCalledTimes(1);

    eTag = 'v2';
    await advanceTimersUntil(provider.updateState());
    expect(mockedWasmResolver.setResolverState).toHaveBeenCalledTimes(2);
  });
  it('retries resolverStateUri on 5xx/network errors with fast backoff', async () => {
    net.cdn.state.status = 503;
    setTimeout(() => {
      net.cdn.state.status = 200;
    }, 1500);

    await advanceTimersUntil(provider.updateState());

    expect(net.cdn.state.calls).toBeGreaterThan(1);
    expect(mockedWasmResolver.setResolverState).toHaveBeenCalledTimes(1);
  });
  it('retries state download with backoff and stall-timeout', async () => {
    await advanceTimersUntil(provider.initialize());
    mockedWasmResolver.setResolverState.mockClear();
    let chunkDelay = 1500;
    net.cdn.state.handler = req => {
      const encrypted = encryptTestState(new Uint8Array(1000));
      const body = new ReadableStream<Uint8Array>({
        async start(controller) {
          for (let i = 0; i < encrypted.length; i += 100) {
            await abortableSleep(chunkDelay, req.signal);
            controller.enqueue(encrypted.slice(i, i + 100));
          }
          controller.close();
        },
      });
      return new Response(body);
    };
    // Decrease chunkDelay after a few retries so next retry succeeds
    setTimeout(() => {
      chunkDelay = 100;
    }, 6000);

    await advanceTimersUntil(provider.updateState());
    expect(net.cdn.state.calls).toBeGreaterThan(1);
    expect(mockedWasmResolver.setResolverState).toHaveBeenCalledTimes(1);
  });
});

describe('flush behavior', () => {
  it('preserves existing telemetry and sets provider SDK metadata when adding provider init telemetry', async () => {
    let sentBody: Uint8Array | undefined;
    net.resolver.flagLogs.handler = async (req: Request) => {
      sentBody = new Uint8Array(await req.arrayBuffer());
      return new Response(null, { status: 200 });
    };
    mockedWasmResolver.flushLogs.mockReturnValueOnce(
      WriteFlagLogsRequest.encode(
        WriteFlagLogsRequest.create({
          telemetryData: {
            resolverVersion: '0.20.0',
            sdk: { id: 25, version: 'resolver-version' },
            providerInitRate: [{ count: 2, labels: { existing: 'true' } }],
          },
        }),
      ).finish(),
    );

    await advanceTimersUntil(provider.flush());

    expect(sentBody).toBeDefined();
    const decoded = WriteFlagLogsRequest.decode(sentBody!);
    expect(decoded.telemetryData?.resolverVersion).toBe('0.20.0');
    expect(decoded.telemetryData?.sdk).toEqual({ id: 22, customId: undefined, version: VERSION });
    expect(decoded.telemetryData?.providerInitRate).toEqual([
      { count: 2, labels: { existing: 'true' } },
      { count: 1, labels: { encryption: 'true' } },
    ]);
  });

  it('does not retry provider init telemetry after an HTTP failure', async () => {
    const sentBodies: Uint8Array[] = [];
    let attempts = 0;
    net.resolver.flagLogs.handler = async (req: Request) => {
      sentBodies.push(new Uint8Array(await req.arrayBuffer()));
      attempts++;
      return new Response(null, { status: attempts <= 3 ? 503 : 200 });
    };
    mockedWasmResolver.flushLogs.mockReturnValue(
      WriteFlagLogsRequest.encode(
        WriteFlagLogsRequest.create({ telemetryData: { resolverVersion: '0.20.0' } }),
      ).finish(),
    );

    await advanceTimersUntil(provider.flush());
    await advanceTimersUntil(provider.flush());

    const firstAttempt = WriteFlagLogsRequest.decode(sentBodies[0]);
    const retryAttempt = WriteFlagLogsRequest.decode(sentBodies[3]);
    expect(firstAttempt.telemetryData?.providerInitRate).toHaveLength(1);
    expect(retryAttempt.telemetryData?.providerInitRate).toHaveLength(0);
  });

  it('flushes periodically at the configured interval', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    const start = net.resolver.flagLogs.calls;

    await vi.advanceTimersByTimeAsync(DEFAULT_FLUSH_INTERVAL);
    expect(net.resolver.flagLogs.calls).toBe(start + 1);

    // +1 periodic flush, +1 pre-state-update flush (state interval = 2x flush interval)
    await vi.advanceTimersByTimeAsync(DEFAULT_FLUSH_INTERVAL);
    expect(net.resolver.flagLogs.calls).toBe(start + 3);
  });
  it('retries flagLogs writes up to 3 attempts', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    // Make writes fail transiently, then succeed
    net.resolver.flagLogs.status = 503;

    const start = net.resolver.flagLogs.calls;
    const startTime = Date.now();
    await advanceTimersUntil(provider.flush());

    const attempts = net.resolver.flagLogs.calls - start;
    expect(attempts).toBe(3);
    expect(Date.now() - startTime).toBe(1500);
  });
  it('does one final flush on close', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    const start = net.resolver.flagLogs.calls;

    await advanceTimersUntil(expect(provider.onClose()).resolves.toBeUndefined());

    expect(net.resolver.flagLogs.calls).toBe(start + 1);
  });
  it('emits provider init telemetry on close when there are no resolver logs', async () => {
    let sentBody: Uint8Array | undefined;
    net.resolver.flagLogs.handler = async (req: Request) => {
      sentBody = new Uint8Array(await req.arrayBuffer());
      return new Response(null, { status: 200 });
    };
    mockedWasmResolver.flushLogs.mockReturnValueOnce(new Uint8Array(0));

    await advanceTimersUntil(expect(provider.onClose()).resolves.toBeUndefined());

    expect(sentBody).toBeDefined();
    const decoded = WriteFlagLogsRequest.decode(sentBody!);
    expect(decoded.telemetryData?.sdk).toEqual({ id: 22, customId: undefined, version: VERSION });
    expect(decoded.telemetryData?.providerInitRate).toEqual([{ count: 1, labels: { encryption: 'true' } }]);
  });
  it('keeps close best-effort when provider init telemetry cannot be sent', async () => {
    mockedWasmResolver.flushLogs.mockReturnValueOnce(new Uint8Array(0));
    net.resolver.flagLogs.status = 'No network';

    await advanceTimersUntil(expect(provider.onClose()).resolves.toBeUndefined());
  });
  it('skips flush if there are no logs to send', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    const start = net.resolver.flagLogs.calls;
    // Make resolver return no logs
    mockedWasmResolver.flushLogs.mockReturnValueOnce(new Uint8Array(0));

    await advanceTimersUntil(provider.flush());

    expect(net.resolver.flagLogs.calls).toBe(start);
  });
});

describe('timeouts and aborts', () => {
  it('recovers in the background if state is not fetched before initializeTimeout', async () => {
    // Make resolverStateUri unreachable so initialize must rely on initializeTimeout
    net.cdn.state.status = 'No network';

    const shortTimeoutProvider = new ConfidenceServerProviderLocal(mockedWasmResolver, noopEventTracker, {
      flagClientSecret: 'flagClientSecret',
      encryptionKey: '00'.repeat(32),
      initializeTimeout: 1000,
      fetch: net.fetch,
    });

    await advanceTimersUntil(
      expect(shortTimeoutProvider.initialize()).rejects.toMatchObject({ code: 'PROVIDER_NOT_READY' }),
    );

    expect(Date.now()).toBe(1000);
    expect(shortTimeoutProvider.status).toBe('ERROR');

    net.cdn.state.status = 200;
    await vi.advanceTimersByTimeAsync(NOT_READY_STATE_INTERVAL);
    await vi.waitFor(() => expect(shortTimeoutProvider.status).toBe('READY'));

    const callsAfterRecovery = net.cdn.state.calls;
    await vi.advanceTimersByTimeAsync(DEFAULT_STATE_INTERVAL - 1000);
    expect(net.cdn.state.calls).toBe(callsAfterRecovery);
    await vi.advanceTimersByTimeAsync(1000);
    expect(net.cdn.state.calls).toBe(callsAfterRecovery + 1);

    await advanceTimersUntil(shortTimeoutProvider.onClose());
  });
  it('returns the default with a provider-not-ready error before recovery', async () => {
    net.cdn.state.status = 'No network';

    const initialization = expect(OpenFeature.setProviderAndWait(provider)).rejects.toMatchObject({
      code: 'PROVIDER_NOT_READY',
    });
    await vi.advanceTimersByTimeAsync(DEFAULT_INITIALIZE_TIMEOUT);
    await initialization;

    await expect(OpenFeature.getClient().getBooleanDetails('flag.enabled', true)).resolves.toEqual(
      expect.objectContaining({
        value: true,
        reason: 'ERROR',
        errorCode: 'PROVIDER_NOT_READY',
      }),
    );
    expect(mockedWasmResolver.resolveProcess).not.toHaveBeenCalled();

    await advanceTimersUntil(OpenFeature.clearProviders());
  });
  it('aborts in-flight state update when provider is closed', async () => {
    // Make state fetch slow so initialize is in-flight
    net.cdn.state.latency = 10_000;

    const init = provider.initialize();
    // Abort provider immediately
    const close = provider.onClose();

    await advanceTimersUntil(expect(init).rejects.toMatchObject({ code: 'PROVIDER_NOT_READY' }));
    await advanceTimersUntil(close);
    expect(provider.status).toBe('ERROR');
    const callsAfterClose = net.cdn.state.calls;
    await vi.runAllTimersAsync();
    expect(net.cdn.state.calls).toBe(callsAfterClose);
  });

  it('handles post-dispatch latency aborts (endpoint invoked)', async () => {
    // Ensure no server latency; abort during endpoint processing
    net.cdn.state.latency = 200;
    const signal = timeoutSignal(100);
    await advanceTimersUntil(expect(provider.updateState(signal)).rejects.toThrow());
    // endpoint was invoked once
    expect(net.cdn.state.calls).toBe(1);
  });
});

describe('OpenFeature startup lifecycle', () => {
  it.each([404, 304])('does not become ready on initial HTTP %s and recovers', async status => {
    net.cdn.state.status = status;
    const init = provider.initialize();
    await vi.advanceTimersByTimeAsync(2000);
    expect(provider.status).toBe(ProviderStatus.NOT_READY);
    expect(mockedWasmResolver.setResolverState).not.toHaveBeenCalled();
    net.cdn.state.status = 200;
    await advanceTimersUntil(init);
    expect(provider.status).toBe(ProviderStatus.READY);
    await advanceTimersUntil(provider.onClose());
  });

  it('stops background startup recovery if credentials are rejected after a timeout', async () => {
    net.cdn.state.status = 'No network';
    const client = OpenFeature.getClient('late-fatal');
    try {
      await advanceTimersUntil(
        expect(OpenFeature.setProviderAndWait('late-fatal', provider)).rejects.toMatchObject({
          code: 'PROVIDER_NOT_READY',
        }),
      );
      net.cdn.state.status = 401;
      await vi.advanceTimersByTimeAsync(NOT_READY_STATE_INTERVAL);
      expect(client.providerStatus).toBe(ProviderStatus.FATAL);
      const calls = net.cdn.state.calls;
      await vi.advanceTimersByTimeAsync(5000);
      expect(net.cdn.state.calls).toBe(calls);
      expect(mockedWasmResolver.setResolverState).not.toHaveBeenCalled();
    } finally {
      await advanceTimersUntil(OpenFeature.clearProviders());
    }
  });

  it.each([undefined, 'startup-recovery'])('recovers after the full budget for domain %s', async domain => {
    net.cdn.state.status = 'No network';
    const client = domain ? OpenFeature.getClient(domain) : OpenFeature.getClient();
    const ready = vi.fn();
    const error = vi.fn();
    client.addHandler(ProviderEvents.Ready, ready);
    client.addHandler(ProviderEvents.Error, error);
    let settled = false;
    const init = domain ? OpenFeature.setProviderAndWait(domain, provider) : OpenFeature.setProviderAndWait(provider);
    const checked = expect(init.finally(() => (settled = true))).rejects.toMatchObject({ code: 'PROVIDER_NOT_READY' });
    try {
      await vi.advanceTimersByTimeAsync(DEFAULT_INITIALIZE_TIMEOUT - 1);
      expect(settled).toBe(false);
      expect(client.providerStatus).toBe(ProviderStatus.NOT_READY);
      expect(net.cdn.state.calls).toBeGreaterThan(20);
      expect(ready).not.toHaveBeenCalled();

      await vi.advanceTimersByTimeAsync(1);
      await checked;
      expect(Date.now()).toBe(DEFAULT_INITIALIZE_TIMEOUT);
      expect(client.providerStatus).toBe(ProviderStatus.ERROR);
      expect(provider.status).toBe(ProviderStatus.ERROR);
      expect(error).toHaveBeenCalledTimes(1);
      expect(ready).not.toHaveBeenCalled();
      await expect(client.getBooleanDetails('flag.enabled', true)).resolves.toMatchObject({
        value: true,
        errorCode: 'PROVIDER_NOT_READY',
      });
      expect(mockedWasmResolver.resolveProcess).not.toHaveBeenCalled();

      net.cdn.state.status = 200;
      await vi.advanceTimersByTimeAsync(NOT_READY_STATE_INTERVAL);
      await vi.waitFor(() => expect(client.providerStatus).toBe(ProviderStatus.READY));
      expect(provider.status).toBe(ProviderStatus.READY);
      expect(mockedWasmResolver.setResolverState).toHaveBeenCalledTimes(1);
      expect(ready).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(DEFAULT_STATE_INTERVAL);
      expect(ready).toHaveBeenCalledTimes(1);
    } finally {
      client.removeHandler(ProviderEvents.Ready, ready);
      client.removeHandler(ProviderEvents.Error, error);
      await advanceTimersUntil(OpenFeature.clearProviders());
    }
  });

  it('finishes early and announces readiness once when startup retries succeed', async () => {
    net.cdn.state.status = 503;
    const client = OpenFeature.getClient('early-recovery');
    const ready = vi.fn();
    client.addHandler(ProviderEvents.Ready, ready);
    const init = OpenFeature.setProviderAndWait('early-recovery', provider);
    try {
      await vi.advanceTimersByTimeAsync(5000);
      expect(client.providerStatus).toBe(ProviderStatus.NOT_READY);
      net.cdn.state.status = 200;
      await advanceTimersUntil(init);
      expect(Date.now()).toBeLessThan(DEFAULT_INITIALIZE_TIMEOUT);
      expect(client.providerStatus).toBe(ProviderStatus.READY);
      expect(ready).toHaveBeenCalledTimes(1);
    } finally {
      client.removeHandler(ProviderEvents.Ready, ready);
      await advanceTimersUntil(OpenFeature.clearProviders());
    }
  });

  it.each([400, 401, 403, 422, 'invalid', 'decode', 'rejected'])(
    'fails promptly without retrying terminal startup failure %s',
    async failure => {
      if (typeof failure === 'number') net.cdn.state.status = failure;
      else if (failure === 'invalid') net.cdn.state.handler = () => new Response(new Uint8Array([1, 2, 3]));
      else if (failure === 'decode')
        net.cdn.state.handler = () => new Response(encryptTestState(new Uint8Array([255])));
      else
        mockedWasmResolver.setResolverState.mockImplementationOnce(() => {
          throw new Error('Rejected state');
        });
      const client = OpenFeature.getClient('fatal');
      const ready = vi.fn();
      client.addHandler(ProviderEvents.Ready, ready);
      try {
        await advanceTimersUntil(
          expect(OpenFeature.setProviderAndWait('fatal', provider)).rejects.toMatchObject({
            code: 'PROVIDER_FATAL',
          }),
        );
        expect(Date.now()).toBeLessThan(DEFAULT_INITIALIZE_TIMEOUT);
        expect(client.providerStatus).toBe(ProviderStatus.FATAL);
        expect(provider.status).toBe(ProviderStatus.FATAL);
        await expect(client.getBooleanDetails('flag.enabled', true)).resolves.toMatchObject({
          value: true,
          errorCode: 'PROVIDER_FATAL',
        });
        await vi.advanceTimersByTimeAsync(DEFAULT_INITIALIZE_TIMEOUT);
        expect(net.cdn.state.calls).toBe(1);
        expect(ready).not.toHaveBeenCalled();
      } finally {
        client.removeHandler(ProviderEvents.Ready, ready);
        await advanceTimersUntil(OpenFeature.clearProviders());
      }
    },
  );

  it('cancels background recovery when closed after a timeout', async () => {
    net.cdn.state.status = 'No network';
    const ready = vi.fn();
    provider.events.addHandler(ProviderEvents.Ready, ready);
    await advanceTimersUntil(expect(provider.initialize()).rejects.toMatchObject({ code: 'PROVIDER_NOT_READY' }));
    await advanceTimersUntil(provider.onClose());
    net.cdn.state.status = 200;
    const callsAfterClose = net.cdn.state.calls;
    await vi.runAllTimersAsync();
    expect(net.cdn.state.calls).toBe(callsAfterClose);
    expect(ready).not.toHaveBeenCalled();
    expect(provider.status).toBe(ProviderStatus.ERROR);
  });

  it('bounds pending decryption and does not install state from an expired attempt', async () => {
    let finishDecrypt!: (value: ArrayBuffer) => void;
    vi.mocked(crypto.subtle.decrypt).mockImplementationOnce(() => new Promise(resolve => (finishDecrypt = resolve)));
    await advanceTimersUntil(expect(provider.initialize()).rejects.toMatchObject({ code: 'PROVIDER_NOT_READY' }));
    expect(Date.now()).toBe(DEFAULT_INITIALIZE_TIMEOUT);
    finishDecrypt(new ArrayBuffer(0));
    await vi.advanceTimersByTimeAsync(0);
    expect(mockedWasmResolver.setResolverState).not.toHaveBeenCalled();
    expect(provider.status).toBe(ProviderStatus.ERROR);
    await vi.advanceTimersByTimeAsync(NOT_READY_STATE_INTERVAL);
    await vi.waitFor(() => expect(provider.status).toBe(ProviderStatus.READY));
    expect(mockedWasmResolver.setResolverState).toHaveBeenCalledTimes(1);
    await advanceTimersUntil(provider.onClose());
  });
});

describe('cached state freshness through OpenFeature', () => {
  it('commits metadata and ETag only after the resolver accepts an update', async () => {
    let account = 'accepted';
    const acceptedDestinations = [LogDestination.LOG_DESTINATION_SPOTIFY_EDGE];
    let logDestinations = acceptedDestinations;
    const etags: Array<string | null> = [];
    net.cdn.state.handler = req => {
      etags.push(req.headers.get('If-None-Match'));
      const state = ClientResolverState.encode(
        ClientResolverState.create({
          state: new Uint8Array(100),
          account,
          logDestinations,
        }),
      ).finish();
      return new Response(encryptTestState(state), { headers: { ETag: account } });
    };
    await advanceTimersUntil(provider.initialize());
    account = 'rejected';
    logDestinations = [];
    mockedWasmResolver.setResolverState.mockImplementationOnce(() => {
      throw new Error('Rejected update');
    });
    await advanceTimersUntil(expect(provider.updateState()).rejects.toThrow('Rejected update'));
    expect(provider).toMatchObject({
      status: ProviderStatus.READY,
      accountId: 'accepted',
      logDestinations: acceptedDestinations,
      stateEtag: 'accepted',
    });
    // The next conditional request must still validate the accepted version.
    net.cdn.state.handler = req => {
      etags.push(req.headers.get('If-None-Match'));
      return new Response(null, { status: 304 });
    };
    await advanceTimersUntil(provider.updateState());
    expect(etags).toEqual([null, 'accepted', 'accepted']);
    await advanceTimersUntil(provider.onClose());
  });

  it.each([0, -1, NaN, Infinity, 1.5])('rejects invalid maxStateAge %s', maxStateAge => {
    expect(
      () =>
        new ConfidenceServerProviderLocal(mockedWasmResolver, noopEventTracker, {
          flagClientSecret: 'secret',
          encryptionKey: '00'.repeat(32),
          maxStateAge,
        }),
    ).toThrow('maxStateAge');
  });

  it.each([undefined, 'stale-cache'])('keeps cached evaluations usable during failures for domain %s', async domain => {
    provider = new ConfidenceServerProviderLocal(mockedWasmResolver, noopEventTracker, {
      flagClientSecret: 'secret',
      encryptionKey: '00'.repeat(32),
      fetch: net.fetch,
      stateUpdateInterval: 1000,
      maxStateAge: 2000,
    });
    mockedWasmResolver.resolveProcess.mockReturnValue({
      resolved: {
        response: {
          resolvedFlags: [
            {
              flag: 'flags/flag',
              variant: 'on',
              value: { enabled: true },
              reason: ResolveReason.RESOLVE_REASON_MATCH,
              shouldApply: false,
              assignmentOrigin: '',
            },
          ],
          resolveToken: new Uint8Array(),
          resolveId: 'id',
        },
        materializationsToWrite: [],
      },
    });
    const client = domain ? OpenFeature.getClient(domain) : OpenFeature.getClient();
    const stale = vi.fn();
    const ready = vi.fn();
    client.addHandler(ProviderEvents.Stale, stale);
    client.addHandler(ProviderEvents.Ready, ready);
    try {
      await advanceTimersUntil(
        domain ? OpenFeature.setProviderAndWait(domain, provider) : OpenFeature.setProviderAndWait(provider),
      );
      // Authentication errors after startup keep the good state.
      net.cdn.state.status = 403;
      await vi.advanceTimersByTimeAsync(1999);
      expect(client.providerStatus).toBe(ProviderStatus.READY);
      await vi.advanceTimersByTimeAsync(1);
      expect(client.providerStatus).toBe(ProviderStatus.STALE);
      expect(stale).toHaveBeenCalledTimes(1);
      await expect(client.getBooleanDetails('flag.enabled', false)).resolves.toMatchObject({ value: true });
      // Invalid refresh payloads also leave cached evaluation and freshness untouched.
      net.cdn.state.status = 200;
      const validHandler = net.cdn.state.handler;
      net.cdn.state.handler = () => new Response(new Uint8Array([1, 2, 3]));
      await vi.advanceTimersByTimeAsync(2000);
      expect(client.providerStatus).toBe(ProviderStatus.STALE);
      await expect(client.getBooleanDetails('flag.enabled', false)).resolves.toMatchObject({ value: true });
      expect(mockedWasmResolver.setResolverState).toHaveBeenCalledTimes(1);
      net.cdn.state.handler = validHandler;
      net.cdn.state.status = 304;
      await vi.advanceTimersByTimeAsync(1000);
      expect(client.providerStatus).toBe(ProviderStatus.READY);
      expect(ready).toHaveBeenCalledTimes(2);
      expect(mockedWasmResolver.setResolverState).toHaveBeenCalledTimes(1);
      // A 304 extends freshness, but an in-flight network retry does not.
      net.cdn.state.status = 'No network';
      await vi.advanceTimersByTimeAsync(1999);
      expect(client.providerStatus).toBe(ProviderStatus.READY);
      await vi.advanceTimersByTimeAsync(1);
      expect(client.providerStatus).toBe(ProviderStatus.STALE);
      expect(stale).toHaveBeenCalledTimes(2);
      await expect(client.getBooleanDetails('flag.enabled', false)).resolves.toMatchObject({ value: true });
    } finally {
      client.removeHandler(ProviderEvents.Stale, stale);
      client.removeHandler(ProviderEvents.Ready, ready);
      await advanceTimersUntil(OpenFeature.clearProviders());
    }
  });
});

describe('network error modes', () => {
  it.each([408, 429, 503])('retries transient HTTP %s responses', async status => {
    net.cdn.state.status = status;
    setTimeout(() => {
      net.cdn.state.status = 200;
    }, 1500);
    await advanceTimersUntil(provider.updateState());
    expect(net.cdn.state.calls).toBeGreaterThan(1);
  });

  it('treats DNS/connect/TLS failures as throws and retries appropriately', async () => {
    net.resolver.flagLogs.status = 'No network';
    await advanceTimersUntil(expect(provider.flush()).rejects.toThrow());
    expect(net.resolver.flagLogs.calls).toBeGreaterThan(1);
  });
});

describe('remote materialization for sticky assignments', () => {
  const RESOLVE_REASON_MATCH = 1;

  it('resolves locally when WASM has all materialization data', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    // WASM resolver succeeds with local data
    mockedWasmResolver.resolveProcess.mockReturnValue({
      resolved: {
        response: {
          resolvedFlags: [
            {
              flag: 'flags/test-flag',
              variant: 'variant-a',
              value: { enabled: true },
              reason: RESOLVE_REASON_MATCH,
              shouldApply: true,
              assignmentOrigin: 'flags/test-flag/rules/rule1',
            },
          ],
          resolveToken: new Uint8Array(),
          resolveId: 'resolve-123',
        },
        materializationsToWrite: [],
      },
    });

    const result = await provider.resolveBooleanEvaluation('test-flag.enabled', false, {
      targetingKey: 'user-123',
    });

    expect(result.value).toBe(true);
    expect(result.variant).toBe('variant-a');

    expect(mockedWasmResolver.resolveProcess).toHaveBeenCalledWith({
      deferredMaterializations: expect.objectContaining({
        flags: ['flags/test-flag'],
        clientSecret: 'flagClientSecret',
      }),
    });

    // No remote call needed
    expect(net.resolver.readMaterializations.calls).toBe(0);
  });

  it('sets apply=false when _confidence_skip_apply is true in context', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    mockedWasmResolver.resolveProcess.mockReturnValue({
      resolved: {
        response: {
          resolvedFlags: [
            {
              flag: 'flags/test-flag',
              variant: 'variant-a',
              value: { enabled: true },
              reason: RESOLVE_REASON_MATCH,
              shouldApply: true,
              assignmentOrigin: 'flags/test-flag/rules/rule1',
            },
          ],
          resolveToken: new Uint8Array(),
          resolveId: 'resolve-123',
        },
        materializationsToWrite: [],
      },
    });

    await provider.resolveBooleanEvaluation('test-flag.enabled', false, {
      targetingKey: 'user-123',
      _confidence_skip_apply: true,
    });

    expect(mockedWasmResolver.resolveProcess).toHaveBeenCalledWith({
      deferredMaterializations: expect.objectContaining({
        apply: false,
        evaluationContext: expect.not.objectContaining({
          _confidence_skip_apply: true,
        }),
      }),
    });
  });

  it('sets apply=false when disableExposureCollection is configured on the provider', async () => {
    provider = new ConfidenceServerProviderLocal(mockedWasmResolver, noopEventTracker, {
      flagClientSecret: 'flagClientSecret',
      encryptionKey: '00'.repeat(32),
      fetch: net.fetch,
      materializationStore: 'CONFIDENCE_REMOTE_STORE',
      disableExposureCollection: true,
    });
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());
    mockedWasmResolver.flushLogs.mockClear();
    mockedWasmResolver.flushAssigned.mockClear();

    mockedWasmResolver.resolveProcess.mockReturnValue({
      resolved: {
        response: {
          resolvedFlags: [
            {
              flag: 'flags/test-flag',
              variant: 'variant-a',
              value: { enabled: true },
              reason: RESOLVE_REASON_MATCH,
              shouldApply: true,
              assignmentOrigin: 'flags/test-flag/rules/rule1',
            },
          ],
          resolveToken: new Uint8Array(),
          resolveId: 'resolve-123',
        },
        materializationsToWrite: [],
      },
    });

    await provider.resolveBooleanEvaluation('test-flag.enabled', false, {
      targetingKey: 'user-123',
    });

    expect(mockedWasmResolver.resolveProcess).toHaveBeenCalledWith({
      deferredMaterializations: expect.objectContaining({
        apply: false,
      }),
    });
    expect(mockedWasmResolver.setResolverState).toHaveBeenCalledWith(
      expect.objectContaining({ disableExposureCollection: true }),
    );
    expect(mockedWasmResolver.flushAssigned).not.toHaveBeenCalled();

    await advanceTimersUntil(expect(provider.onClose()).resolves.toBeUndefined());
    expect(mockedWasmResolver.flushLogs).toHaveBeenCalled();
    expect(mockedWasmResolver.flushAssigned).not.toHaveBeenCalled();
  });

  it('reads materializations from remote when WASM reports missing materializations', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    // WASM resolver reports missing materialization (suspended)
    mockedWasmResolver.resolveProcess.mockReturnValueOnce({
      suspended: {
        materializationsToRead: [{ unit: 'user-456', rule: 'rule-1', materialization: 'mat-v1', variant: '' }],
        state: new Uint8Array([1, 2, 3]),
      },
    });
    mockedWasmResolver.resolveProcess.mockReturnValueOnce({
      resolved: {
        materializationsToWrite: [],
        response: {
          resolvedFlags: [
            {
              flag: 'flags/my-flag',
              variant: 'flags/my-flag/variants/control',
              value: { color: 'blue', size: 10 },
              reason: ResolveReason.RESOLVE_REASON_MATCH,
              shouldApply: true,
              assignmentOrigin: 'flags/my-flag/rules/rule1',
            },
          ],
          resolveToken: new Uint8Array(),
          resolveId: 'remote-resolve-456',
        },
      },
    });

    const result = await provider.resolveObjectEvaluation(
      'my-flag',
      { color: 'red' },
      {
        targetingKey: 'user-456',
        country: 'SE',
      },
    );
    expect(result.reason).toEqual('MATCH');
    expect(result.variant).toBe('flags/my-flag/variants/control');

    // Remote store should have been called
    expect(net.resolver.readMaterializations.calls).toBe(1);
  });

  it('retries remote read materialization on transient errors', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    mockedWasmResolver.resolveProcess.mockReturnValueOnce({
      suspended: {
        materializationsToRead: [{ unit: 'user-1', rule: 'rule-1', materialization: 'mat-1', variant: '' }],
        state: new Uint8Array([1, 2, 3]),
      },
    });
    mockedWasmResolver.resolveProcess.mockReturnValueOnce({
      resolved: {
        materializationsToWrite: [],
        response: {
          resolvedFlags: [
            {
              flag: 'flags/test-flag',
              variant: 'flags/my-flag/variants/control',
              value: { ok: true },
              reason: ResolveReason.RESOLVE_REASON_MATCH,
              shouldApply: true,
              assignmentOrigin: 'flags/test-flag/rules/rule1',
            },
          ],
          resolveToken: new Uint8Array(),
          resolveId: 'remote-resolve-456',
        },
      },
    });

    // First two calls fail, third succeeds
    net.resolver.readMaterializations.status = 503;
    setTimeout(() => {
      net.resolver.readMaterializations.status = 200;
    }, 300);

    const result = await advanceTimersUntil(
      provider.resolveBooleanEvaluation('test-flag.ok', false, { targetingKey: 'user-1' }),
    );

    expect(result.value).toBe(true);
    // Should have retried multiple times
    expect(net.resolver.readMaterializations.calls).toBeGreaterThan(1);
    expect(net.resolver.readMaterializations.calls).toBeLessThanOrEqual(3);
  });

  it('writes materializations to remote when WASM reports materialization updates', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    mockedWasmResolver.resolveProcess.mockReturnValueOnce({
      resolved: {
        materializationsToWrite: [{ unit: 'u1', materialization: 'm1', rule: 'r1', variant: 'v1' }],
        response: {
          resolvedFlags: [
            {
              flag: 'flags/test-flag',
              variant: 'flags/my-flag/variants/control',
              value: { ok: true },
              reason: ResolveReason.RESOLVE_REASON_MATCH,
              shouldApply: true,
              assignmentOrigin: 'flags/test-flag/rules/rule1',
            },
          ],
          resolveToken: new Uint8Array(),
          resolveId: 'remote-resolve-456',
        },
      },
    });

    await advanceTimersUntil(
      expect(provider.resolveBooleanEvaluation('test-flag.ok', false, { targetingKey: 'user-1' })).resolves.toEqual(
        expect.objectContaining({ value: true }),
      ),
    );

    // SDK doesn't wait for writes so need we need to wait here.
    await advanceTimersUntil(() => net.resolver.writeMaterializations.calls === 1);
  });
});

describe('SDK telemetry', () => {
  const RESOLVE_REASON_MATCH = 1;

  it('includes SDK id and version in resolve requests', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    // WASM resolver succeeds with local data
    mockedWasmResolver.resolveProcess.mockReturnValue({
      resolved: {
        response: {
          resolvedFlags: [
            {
              flag: 'test-flag',
              variant: 'variant-a',
              value: { enabled: true },
              reason: RESOLVE_REASON_MATCH,
              shouldApply: true,
              assignmentOrigin: 'flags/test-flag/rules/rule1',
            },
          ],
          resolveToken: new Uint8Array(),
          resolveId: 'resolve-123',
        },
        materializationsToWrite: [],
      },
    });

    await provider.resolveBooleanEvaluation('test-flag.enabled', false, {
      targetingKey: 'user-123',
    });

    // Verify SDK information is included in the resolve request
    expect(mockedWasmResolver.resolveProcess).toHaveBeenCalledWith(
      expect.objectContaining({
        deferredMaterializations: expect.objectContaining({
          sdk: expect.objectContaining({
            id: 22, // SDK_ID_JS_LOCAL_SERVER_PROVIDER
            version: expect.stringMatching(/^\d+\.\d+\.\d+$/), // Semantic version format
          }),
        }),
      }),
    );
  });
});

describe('registerResolve telemetry', () => {
  const RESOLVE_REASON_MATCH = 1;

  it('calls registerResolve with MATCH reason after successful resolve', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    mockedWasmResolver.resolveProcess.mockReturnValue({
      resolved: {
        response: {
          resolvedFlags: [
            {
              flag: 'flags/test-flag',
              variant: 'variant-a',
              value: { enabled: true },
              reason: RESOLVE_REASON_MATCH,
              shouldApply: true,
              assignmentOrigin: 'flags/test-flag/rules/rule1',
            },
          ],
          resolveToken: new Uint8Array(),
          resolveId: 'resolve-123',
        },
        materializationsToWrite: [],
      },
    });

    await provider.resolveBooleanEvaluation('test-flag.enabled', false, {
      targetingKey: 'user-123',
    });

    expect(mockedWasmResolver.registerResolve).toHaveBeenCalledWith(
      expect.objectContaining({
        reason: ResolveReason.RESOLVE_REASON_MATCH,
        latencyUs: expect.any(Number),
      }),
    );
  });

  it('calls registerResolve with FLAG_NOT_FOUND for missing flags', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    mockedWasmResolver.resolveProcess.mockReturnValue({
      resolved: {
        response: {
          resolvedFlags: [],
          resolveToken: new Uint8Array(),
          resolveId: 'resolve-456',
        },
        materializationsToWrite: [],
      },
    });

    await provider.resolveBooleanEvaluation('missing-flag.enabled', false, {
      targetingKey: 'user-123',
    });

    expect(mockedWasmResolver.registerResolve).toHaveBeenCalledWith(
      expect.objectContaining({
        reason: ResolveReason.RESOLVE_REASON_FLAG_NOT_FOUND,
      }),
    );
  });

  it('calls registerResolve with TYPE_MISMATCH for wrong type', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    mockedWasmResolver.resolveProcess.mockReturnValue({
      resolved: {
        response: {
          resolvedFlags: [
            {
              flag: 'flags/test-flag',
              variant: 'variant-a',
              value: { enabled: 'not-a-boolean' },
              reason: RESOLVE_REASON_MATCH,
              shouldApply: true,
              assignmentOrigin: 'flags/test-flag/rules/rule1',
            },
          ],
          resolveToken: new Uint8Array(),
          resolveId: 'resolve-789',
        },
        materializationsToWrite: [],
      },
    });

    await provider.resolveBooleanEvaluation('test-flag.enabled', false, {
      targetingKey: 'user-123',
    });

    expect(mockedWasmResolver.registerResolve).toHaveBeenCalledWith(
      expect.objectContaining({
        reason: ResolveReason.RESOLVE_REASON_TYPE_MISMATCH,
      }),
    );
  });

  it('calls registerResolve with ERROR reason when resolve itself fails', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    mockedWasmResolver.resolveProcess.mockImplementation(() => {
      throw new Error('Resolver state not set');
    });

    const result = await provider.resolveBooleanEvaluation('test-flag.enabled', false, {
      targetingKey: 'user-123',
    });

    expect(result.errorCode).toBeDefined();
    expect(mockedWasmResolver.registerResolve).toHaveBeenCalledWith(
      expect.objectContaining({
        reason: ResolveReason.RESOLVE_REASON_ERROR,
      }),
    );
  });
});

describe('getPrometheusMetrics', () => {
  it('calls prometheusSnapshot on the resolver and returns the result', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    mockedWasmResolver.prometheusSnapshot.mockReturnValue('# HELP some_metric\nsome_metric 42\n');

    const result = provider.getPrometheusMetrics();

    expect(mockedWasmResolver.prometheusSnapshot).toHaveBeenCalledWith('0');
    expect(result).toBe('# HELP some_metric\nsome_metric 42\n');
  });
});

describe('mandatory encryption', () => {
  afterEach(() => vi.useFakeTimers());
  it.each([undefined, null, '', ' ', '00'.repeat(31), '00'.repeat(33), 'gg'.repeat(32), '00'.repeat(32) + '\n'])(
    'rejects an invalid key before fetching: %s',
    encryptionKey => {
      expect(
        () =>
          new ConfidenceServerProviderLocal(mockedWasmResolver, noopEventTracker, {
            flagClientSecret: 'secret',
            encryptionKey: encryptionKey as string,
            fetch: net.fetch,
          }),
      ).toThrow('64 hexadecimal');
      expect(net.cdn.state.calls).toBe(0);
    },
  );

  it.each(['ab'.repeat(32), 'AB'.repeat(32)])('accepts a valid key: %s', encryptionKey => {
    expect(
      () =>
        new ConfidenceServerProviderLocal(mockedWasmResolver, noopEventTracker, {
          flagClientSecret: 'secret',
          encryptionKey,
          fetch: net.fetch,
        }),
    ).not.toThrow();
  });

  it.each(['wrong-key', 'tampered', 'truncated', 'plaintext'])(
    'retries encrypted state after %s without caching its ETag',
    async failure => {
      vi.useRealTimers();
      const { ClientResolverState } = await import('./proto/confidence/flags/admin/v1/resolver');
      const { createCipheriv } = await import('node:crypto');
      const plaintext = ClientResolverState.encode({
        state: new Uint8Array(100),
        account: 'account',
        logDestinations: [],
      }).finish();
      const encrypted = encryptTestState(plaintext);
      const cipher = createCipheriv('aes-256-gcm', Buffer.alloc(32, 1), Buffer.alloc(12));
      const wrongKey = new Uint8Array(
        Buffer.concat([Buffer.alloc(12), cipher.update(plaintext), cipher.final(), cipher.getAuthTag()]),
      );
      const tampered = encrypted.slice();
      tampered[tampered.length - 1] ^= 1;
      const bad =
        failure === 'wrong-key'
          ? wrongKey
          : failure === 'tampered'
          ? tampered
          : failure === 'truncated'
          ? encrypted.slice(0, 5)
          : plaintext;
      let calls = 0;
      const etags: Array<string | null> = [];
      net.cdn.state.handler = req => {
        expect(new URL(req.url).pathname.endsWith('.enc')).toBe(true);
        etags.push(req.headers.get('If-None-Match'));
        calls++;
        return new Response(calls === 2 ? bad : encrypted, { headers: { ETag: calls === 2 ? 'bad' : 'good' } });
      };
      await provider.updateState();
      await expect(provider.updateState()).rejects.toThrow();
      await provider.updateState();
      expect(etags).toEqual([null, 'good', 'good']);
      expect(mockedWasmResolver.setResolverState).toHaveBeenCalledTimes(2);
    },
  );
});

describe('apply-event dedup', () => {
  it('is enabled when the option is not set', async () => {
    // The shared provider from beforeEach never mentions enableApplyDedup, so
    // this asserts the default that ships — not a value the test supplied.
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    expect(mockedWasmResolver.setResolverState).toHaveBeenCalledWith(
      expect.objectContaining({ enableApplyDedup: true }),
    );
  });

  it('is disabled when the option is set to false', async () => {
    const optedOut = new ConfidenceServerProviderLocal(mockedWasmResolver, noopEventTracker, {
      flagClientSecret: 'flagClientSecret',
      encryptionKey: '00'.repeat(32),
      fetch: net.fetch,
      materializationStore: 'CONFIDENCE_REMOTE_STORE',
      enableApplyDedup: false,
    });

    await advanceTimersUntil(expect(optedOut.initialize()).resolves.toBeUndefined());

    expect(mockedWasmResolver.setResolverState).toHaveBeenCalledWith(
      expect.objectContaining({ enableApplyDedup: false }),
    );

    await advanceTimersUntil(optedOut.onClose());
  });

  it('is enabled when the option is set to true', async () => {
    const optedIn = new ConfidenceServerProviderLocal(mockedWasmResolver, noopEventTracker, {
      flagClientSecret: 'flagClientSecret',
      encryptionKey: '00'.repeat(32),
      fetch: net.fetch,
      materializationStore: 'CONFIDENCE_REMOTE_STORE',
      enableApplyDedup: true,
    });

    await advanceTimersUntil(expect(optedIn.initialize()).resolves.toBeUndefined());

    expect(mockedWasmResolver.setResolverState).toHaveBeenCalledWith(
      expect.objectContaining({ enableApplyDedup: true }),
    );

    await advanceTimersUntil(optedIn.onClose());
  });

  // The tests above construct the provider directly, which is right for them
  // but means they cannot catch a README snippet that calls the wrong entry
  // point — exactly how an uncompilable opt-out example shipped. The README
  // documents `createConfidenceServerProvider({ ... })`, so pin that call
  // shape here. The import is type-only: the real entry point loads WASM at
  // call time and has no place in this suite, but `tsc` still fails if the
  // factory is renamed, its parameter shape changes, or `enableApplyDedup`
  // stops being an accepted option.
  it('typechecks the opt-out exactly as the README documents it', () => {
    type ReadmeFactoryOptions = Parameters<typeof NodeEntry.createConfidenceServerProvider>[0];

    const readmeSnippet: ReadmeFactoryOptions = {
      flagClientSecret: 'your-client-secret',
      encryptionKey: '00'.repeat(32),
      enableApplyDedup: false,
    };

    expect(readmeSnippet.enableApplyDedup).toBe(false);
  });
});
