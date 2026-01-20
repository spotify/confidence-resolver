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
import { ResolveReason } from './proto/confidence/flags/resolver/v1/types';

vi.mock(import('./hash'), async () => {
  const { sha256Hex, sha256Bytes } = await import('./test-helpers');
  return {
    sha256Hex,
    sha256Bytes,
  };
});

const mockedWasmResolver: MockedObject<LocalResolver> = {
  setEncryptionKey: vi.fn(),
  resolveWithSticky: vi.fn(),
  setResolverState: vi.fn(),
  flushLogs: vi.fn().mockReturnValue(new Uint8Array(100)),
  flushAssigned: vi.fn().mockReturnValue(new Uint8Array(50)),
  applyFlags: vi.fn(),
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
    materializationStore: 'CONFIDENCE_REMOTE_STORE',
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

describe('remote materialization for sticky assignments', () => {
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

    expect(mockedWasmResolver.resolveWithSticky).toHaveBeenCalledWith({
      resolveRequest: expect.objectContaining({
        flags: ['flags/test-flag'],
        clientSecret: 'flagClientSecret',
      }),
      materializations: [],
      failFastOnSticky: false,
      notProcessSticky: false,
    });

    // No remote call needed
    expect(net.resolver.readMaterializations.calls).toBe(0);
  });

  it('reads materializations from remote when WASM reports missing materializations', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    // WASM resolver reports missing materialization
    mockedWasmResolver.resolveWithSticky.mockReturnValueOnce({
      readOpsRequest: { ops: [{ variantReadOp: { unit: 'user-456', rule: 'rule-1', materialization: 'mat-v1' } }] },
    });
    mockedWasmResolver.resolveWithSticky.mockReturnValueOnce({
      success: {
        materializationUpdates: [],
        response: {
          resolvedFlags: [
            {
              flag: 'flags/my-flag',
              variant: 'flags/my-flag/variants/control',
              value: { color: 'blue', size: 10 },
              reason: ResolveReason.RESOLVE_REASON_MATCH,
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

    mockedWasmResolver.resolveWithSticky.mockReturnValueOnce({
      readOpsRequest: {
        ops: [{ variantReadOp: { unit: 'user-1', rule: 'rule-1', materialization: 'mat-1' } }],
      },
    });
    mockedWasmResolver.resolveWithSticky.mockReturnValueOnce({
      success: {
        materializationUpdates: [],
        response: {
          resolvedFlags: [
            {
              flag: 'flags/test-flag',
              variant: 'flags/my-flag/variants/control',
              value: { ok: true },
              reason: ResolveReason.RESOLVE_REASON_MATCH,
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

    mockedWasmResolver.resolveWithSticky.mockReturnValueOnce({
      success: {
        materializationUpdates: [{ unit: 'u1', materialization: 'm1', rule: 'r1', variant: 'v1' }],
        response: {
          resolvedFlags: [
            {
              flag: 'flags/test-flag',
              variant: 'flags/my-flag/variants/control',
              value: { ok: true },
              reason: ResolveReason.RESOLVE_REASON_MATCH,
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
    expect(mockedWasmResolver.resolveWithSticky).toHaveBeenCalledWith(
      expect.objectContaining({
        resolveRequest: expect.objectContaining({
          sdk: expect.objectContaining({
            id: 22, // SDK_ID_JS_LOCAL_SERVER_PROVIDER
            version: expect.stringMatching(/^\d+\.\d+\.\d+$/), // Semantic version format
          }),
        }),
      }),
    );
  });
});

describe('createFlagBundle', () => {
  const RESOLVE_REASON_MATCH = 1;

  it('creates a bundle with resolved flags and returns pre-bound applyFlag function', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    const resolveToken = new Uint8Array([1, 2, 3, 4]);
    mockedWasmResolver.resolveWithSticky.mockReturnValue({
      success: {
        response: {
          resolvedFlags: [
            {
              flag: 'flags/feature-a',
              variant: 'flags/feature-a/variants/enabled',
              value: { enabled: true, color: 'blue' },
              reason: RESOLVE_REASON_MATCH,
            },
            {
              flag: 'flags/feature-b',
              variant: 'flags/feature-b/variants/control',
              value: { limit: 100 },
              reason: RESOLVE_REASON_MATCH,
            },
          ],
          resolveToken,
          resolveId: 'resolve-bundle-123',
        },
        materializationUpdates: [],
      },
    });

    const result = await provider.createFlagBundle({ targetingKey: 'user-456' });

    // Verify bundle structure
    expect(result.bundle.resolveId).toBe('resolve-bundle-123');
    expect(result.bundle.resolveToken).toBe('AQIDBA=='); // base64 of [1,2,3,4]
    expect(result.bundle.flags).toEqual({
      'feature-a': {
        value: { enabled: true, color: 'blue' },
        variant: 'flags/feature-a/variants/enabled',
        reason: 'MATCH',
      },
      'feature-b': {
        value: { limit: 100 },
        variant: 'flags/feature-b/variants/control',
        reason: 'MATCH',
      },
    });

    // Verify resolve request was made with apply: false
    expect(mockedWasmResolver.resolveWithSticky).toHaveBeenCalledWith(
      expect.objectContaining({
        resolveRequest: expect.objectContaining({
          apply: false,
          evaluationContext: { targeting_key: 'user-456' },
        }),
      }),
    );

    // Verify applyFlag function works
    result.applyFlag('feature-a');

    expect(mockedWasmResolver.applyFlags).toHaveBeenCalledWith(
      expect.objectContaining({
        flags: [{ flag: 'flags/feature-a', applyTime: expect.any(Date) }],
        clientSecret: 'flagClientSecret',
        resolveToken,
      }),
    );
  });

  it('resolves specific flags when flag names are provided', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    mockedWasmResolver.resolveWithSticky.mockReturnValue({
      success: {
        response: {
          resolvedFlags: [
            {
              flag: 'flags/my-flag',
              variant: 'flags/my-flag/variants/test',
              value: { active: true },
              reason: RESOLVE_REASON_MATCH,
            },
          ],
          resolveToken: new Uint8Array(),
          resolveId: 'resolve-123',
        },
        materializationUpdates: [],
      },
    });

    await provider.createFlagBundle({ targetingKey: 'user-789' }, ['my-flag', 'other-flag']);

    expect(mockedWasmResolver.resolveWithSticky).toHaveBeenCalledWith(
      expect.objectContaining({
        resolveRequest: expect.objectContaining({
          flags: ['flags/my-flag', 'flags/other-flag'],
        }),
      }),
    );
  });
});

describe('applyFlag', () => {
  it('calls resolver.applyFlags with correct parameters', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    const resolveToken = 'dGVzdC10b2tlbg=='; // base64 of "test-token"

    provider.applyFlag(resolveToken, 'my-feature');

    expect(mockedWasmResolver.applyFlags).toHaveBeenCalledWith(
      expect.objectContaining({
        flags: [{ flag: 'flags/my-feature', applyTime: expect.any(Date) }],
        clientSecret: 'flagClientSecret',
        resolveToken: expect.any(Uint8Array),
        sendTime: expect.any(Date),
        sdk: expect.objectContaining({
          id: 22, // SDK_ID_JS_LOCAL_SERVER_PROVIDER
        }),
      }),
    );
  });

  it('correctly decodes base64 resolve token', async () => {
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());

    // base64 of [1, 2, 3, 4]
    const resolveToken = 'AQIDBA==';

    provider.applyFlag(resolveToken, 'test-flag');

    expect(mockedWasmResolver.applyFlags).toHaveBeenCalledWith(
      expect.objectContaining({
        resolveToken: new Uint8Array([1, 2, 3, 4]),
      }),
    );
  });
});
