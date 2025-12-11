import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, MockedObject, test, vi } from 'vitest';
import { LocalResolver } from './LocalResolver';
import {
  ConfidenceServerProviderLocal,
  DEFAULT_FLUSH_INTERVAL,
  DEFAULT_STATE_INTERVAL,
} from './ConfidenceServerProviderLocal';
import { abortableSleep, TimeUnit, timeoutSignal } from './util';
import { advanceTimersUntil, NetworkMock } from './test-helpers';
import { sha256Hex } from './hash';

vi.mock(import('./hash'), async () => {
  const { sha256Hex } = await import('./test-helpers');
  return {
    sha256Hex,
  };
});

const mockedWasmResolver: MockedObject<LocalResolver> = {
  resolveWithSticky: vi.fn(),
  setResolverState: vi.fn(),
  flushLogs: vi.fn().mockReturnValue(new Uint8Array(100)),
  flushAssigned: vi.fn().mockReturnValue(new Uint8Array(50)),
};

let provider: ConfidenceServerProviderLocal;
let net: NetworkMock;

vi.useFakeTimers();

beforeEach(() => {
  vi.clearAllMocks();
  vi.clearAllTimers();
  vi.setSystemTime(0);
  net = new NetworkMock();
  provider = new ConfidenceServerProviderLocal(mockedWasmResolver, {
    flagClientSecret: 'flagClientSecret',
    fetch: net.fetch,
  });
});

afterEach(() => {});

describe('idealized conditions', () => {
  it('makes some requests', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    await vi.advanceTimersByTimeAsync(TimeUnit.HOUR + TimeUnit.SECOND);

    // since we fetch state every 30s we should fetch 120 times, but we also do an initial fetch in initialize
    expect(net.cdn.state.calls).toBe(121);
    // flush is called every 10s so 360 times in an hour
    expect(net.resolver.flagLogs.calls).toBe(360);

    await advanceTimersUntil(expect(provider.onClose()).resolves.toBeUndefined());

    // close does a final flush
    expect(net.resolver.flagLogs.calls).toBe(361);
  });
});

describe('no network', () => {
  beforeEach(() => {
    net.error = 'No network';
  });

  it('initialize throws after timeout', async () => {
    await advanceTimersUntil(expect(provider.initialize()).rejects.toThrow());

    expect(provider.status).toBe('ERROR');
    expect(Date.now()).toBe(DEFAULT_STATE_INTERVAL);
  });
});

describe('state update scheduling', () => {
  it('fetches resolverStateUri on initialize', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());
    expect(net.cdn.state.calls).toBe(1);
  });
  it('polls state at fixed interval', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());
    expect(net.cdn.state.calls).toBe(1);

    await vi.advanceTimersByTimeAsync(DEFAULT_STATE_INTERVAL);
    expect(net.cdn.state.calls).toBe(2);

    await vi.advanceTimersByTimeAsync(DEFAULT_STATE_INTERVAL);
    expect(net.cdn.state.calls).toBe(3);
  });
  it('honors If-None-Match and handles 304 Not Modified', async () => {
    let eTag = 'v1';
    const payload = new Uint8Array(100);
    net.cdn.state.handler = req => {
      const ifNoneMatch = req.headers.get('If-None-Match');
      if (ifNoneMatch === eTag) {
        return new Response(null, { status: 304 });
      }
      return new Response(payload, { headers: { eTag } });
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
    let chunkDelay = 600;
    net.cdn.state.handler = req => {
      const body = new ReadableStream<Uint8Array>({
        async start(controller) {
          for (let i = 0; i < 10; i++) {
            await abortableSleep(chunkDelay, req.signal);
            controller.enqueue(new Uint8Array(100));
          }
          controller.close();
        },
      });
      return new Response(body);
    };
    // Decrease chunkDelay after 2.5s so next retry succeeds
    setTimeout(() => {
      chunkDelay = 100;
    }, 2500);

    await advanceTimersUntil(provider.updateState());
    expect(net.cdn.state.calls).toBeGreaterThan(1);
    expect(mockedWasmResolver.setResolverState).toHaveBeenCalledTimes(1);
  });
});

describe('flush behavior', () => {
  it('flushes periodically at the configured interval', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    const start = net.resolver.flagLogs.calls;

    await vi.advanceTimersByTimeAsync(DEFAULT_FLUSH_INTERVAL);
    expect(net.resolver.flagLogs.calls).toBe(start + 1);

    await vi.advanceTimersByTimeAsync(DEFAULT_FLUSH_INTERVAL);
    expect(net.resolver.flagLogs.calls).toBe(start + 2);
  });
  it('retries flagLogs writes up to 3 attempts', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    // Make writes fail transiently, then succeed
    net.resolver.flagLogs.status = 503;

    const start = net.resolver.flagLogs.calls;
    await advanceTimersUntil(provider.flush());

    const attempts = net.resolver.flagLogs.calls - start;
    expect(attempts).toBe(3);
    expect(Date.now()).toBe(1500);
  });
  it('does one final flush on close', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    const start = net.resolver.flagLogs.calls;

    await advanceTimersUntil(expect(provider.onClose()).resolves.toBeUndefined());

    expect(net.resolver.flagLogs.calls).toBe(start + 1);
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
  it('initialize times out if state not fetched before initializeTimeout', async () => {
    // Make resolverStateUri unreachable so initialize must rely on initializeTimeout
    net.cdn.state.status = 'No network';

    const shortTimeoutProvider = new ConfidenceServerProviderLocal(mockedWasmResolver, {
      flagClientSecret: 'flagClientSecret',
      initializeTimeout: 1000,
      fetch: net.fetch,
    });

    await advanceTimersUntil(expect(shortTimeoutProvider.initialize()).rejects.toThrow());

    expect(Date.now()).toBe(1000);
    expect(shortTimeoutProvider.status).toBe('ERROR');
  });
  it('aborts in-flight state update when provider is closed', async () => {
    // Make state fetch slow so initialize is in-flight
    net.cdn.state.latency = 10_000;

    const init = provider.initialize();
    // Abort provider immediately
    const close = provider.onClose();

    await advanceTimersUntil(expect(init).rejects.toThrow());
    await advanceTimersUntil(close);
    expect(provider.status).toBe('ERROR');
    await vi.runAllTimersAsync();
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

describe('network error modes', () => {
  it('treats HTTP 5xx as Response (no throw) and retries appropriately', async () => {
    net.cdn.state.status = 503;
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

describe('remote resolver fallback for sticky assignments', () => {
  const RESOLVE_REASON_MATCH = 1;

  it('resolves locally when WASM has all materialization data', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    // WASM resolver succeeds with local data
    mockedWasmResolver.resolveWithSticky.mockReturnValue({
      success: {
        response: {
          resolvedFlags: [
            {
              flag: 'test-flag',
              variant: 'variant-a',
              value: { enabled: true },
              reason: RESOLVE_REASON_MATCH,
            },
          ],
          resolveToken: new Uint8Array(),
          resolveId: 'resolve-123',
        },
        materializationUpdates: [],
      },
    });

    const result = await provider.resolveBooleanEvaluation('test-flag.enabled', false, {
      targetingKey: 'user-123',
    });

    expect(result.value).toBe(true);
    expect(result.variant).toBe('variant-a');

    // Should use failFastOnSticky: true (fallback strategy)
    expect(mockedWasmResolver.resolveWithSticky).toHaveBeenCalledWith({
      resolveRequest: expect.objectContaining({
        flags: ['flags/test-flag'],
        clientSecret: 'flagClientSecret',
      }),
      materializations: [],
      failFastOnSticky: true,
      notProcessSticky: false,
    });

    // No remote call needed
    expect(net.resolver.flagsResolve.calls).toBe(0);
  });

  it('falls back to remote resolver when WASM reports missing materializations', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    // WASM resolver reports missing materialization
    mockedWasmResolver.resolveWithSticky.mockReturnValue({
      readOpsRequest: { ops: [{ variantReadOp: { unit: 'user-456', rule: 'rule-1', materialization: 'mat-v1' } }] },
    });

    // Configure remote resolver response
    net.resolver.flagsResolve.handler = () => {
      return new Response(
        JSON.stringify({
          resolvedFlags: [
            {
              flag: 'flags/my-flag',
              variant: 'flags/my-flag/variants/control',
              value: { color: 'blue', size: 10 },
              reason: 'RESOLVE_REASON_MATCH',
            },
          ],
          resolveToken: '',
          resolveId: 'remote-resolve-456',
        }),
        {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        },
      );
    };

    const result = await provider.resolveObjectEvaluation(
      'my-flag',
      { color: 'red' },
      {
        targetingKey: 'user-456',
        country: 'SE',
      },
    );

    expect(result.value).toEqual({ color: 'blue', size: 10 });
    expect(result.variant).toBe('flags/my-flag/variants/control');

    // Remote resolver should have been called
    expect(net.resolver.flagsResolve.calls).toBe(1);

    // Verify auth header was added
    const lastRequest = net.resolver.flagsResolve.requests[0];
    expect(lastRequest.method).toBe('POST');
  });

  it('retries remote resolve on transient errors', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    mockedWasmResolver.resolveWithSticky.mockReturnValue({
      readOpsRequest: {
        ops: [{ variantReadOp: { unit: 'user-1', rule: 'rule-1', materialization: 'mat-1' } }],
      },
    });

    // First two calls fail, third succeeds
    net.resolver.flagsResolve.status = 503;
    setTimeout(() => {
      net.resolver.flagsResolve.status = 200;
      net.resolver.flagsResolve.handler = () =>
        new Response(
          JSON.stringify({
            resolvedFlags: [{ flag: 'test-flag', variant: 'v1', value: { ok: true }, reason: 'RESOLVE_REASON_MATCH' }],
            resolveToken: '',
            resolveId: 'resolved',
          }),
          { status: 200 },
        );
    }, 300);

    const result = await advanceTimersUntil(
      provider.resolveBooleanEvaluation('test-flag.ok', false, { targetingKey: 'user-1' }),
    );

    expect(result.value).toBe(true);
    // Should have retried multiple times
    expect(net.resolver.flagsResolve.calls).toBeGreaterThan(1);
    expect(net.resolver.flagsResolve.calls).toBeLessThanOrEqual(3);
  });
});

describe('SDK telemetry', () => {
  const RESOLVE_REASON_MATCH = 1;

  it('includes SDK id and version in resolve requests', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    // WASM resolver succeeds with local data
    mockedWasmResolver.resolveWithSticky.mockReturnValue({
      success: {
        response: {
          resolvedFlags: [
            {
              flag: 'test-flag',
              variant: 'variant-a',
              value: { enabled: true },
              reason: RESOLVE_REASON_MATCH,
            },
          ],
          resolveToken: new Uint8Array(),
          resolveId: 'resolve-123',
        },
        materializationUpdates: [],
      },
    });

    await provider.resolveBooleanEvaluation('test-flag.enabled', false, {
      targetingKey: 'user-123',
    });

    // Verify SDK information is included in the resolve request
    expect(mockedWasmResolver.resolveWithSticky).toHaveBeenCalledWith({
      resolveRequest: expect.objectContaining({
        flags: ['flags/test-flag'],
        clientSecret: 'flagClientSecret',
        sdk: expect.objectContaining({
          id: 22, // SDK_ID_JS_LOCAL_SERVER_PROVIDER
          version: expect.stringMatching(/^\d+\.\d+\.\d+$/), // Semantic version format
        }),
      }),
      materializations: [],
      failFastOnSticky: true,
      notProcessSticky: false,
    });
  });
});
