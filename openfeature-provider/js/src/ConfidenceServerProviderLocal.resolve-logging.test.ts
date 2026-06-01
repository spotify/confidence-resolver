import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { WasmResolver } from './WasmResolver';
import { ConfidenceServerProviderLocal } from './ConfidenceServerProviderLocal';
import { advanceTimersUntil, NetworkMock } from './test-helpers';
import { WriteFlagLogsRequest } from './proto/test-only';
import { SetResolverStateRequest } from './proto/confidence/wasm/messages';

vi.mock(import('./hash'), async () => {
  const { sha256Hex } = await import('./test-helpers');
  return { sha256Hex };
});

const moduleBytes = readFileSync(__dirname + '/../../../wasm/confidence_resolver.wasm');
const stateBytes = readFileSync(__dirname + '/../../../wasm/resolver_state.pb');
const CLIENT_SECRET = 'mkjJruAATQWjeY7foFIWfVAcBWnci2YF';

vi.useFakeTimers();

describe('flagbundle resolve telemetry', () => {
  let provider: ConfidenceServerProviderLocal;
  let net: NetworkMock;
  let capturedBodies: Uint8Array[];

  beforeEach(async () => {
    vi.clearAllMocks();
    vi.clearAllTimers();
    vi.setSystemTime(0);

    net = new NetworkMock();
    capturedBodies = [];

    net.cdn.state.handler = () =>
      new Response(
        SetResolverStateRequest.encode({
          state: stateBytes,
          accountId: 'confidence-test',
        }).finish(),
      );

    net.resolver.flagLogs.handler = async (req: Request) => {
      const body = new Uint8Array(await req.arrayBuffer());
      capturedBodies.push(body);
      return new Response(null, { status: 200 });
    };

    const module = new WebAssembly.Module(moduleBytes);
    const resolver = new WasmResolver(module);

    provider = new ConfidenceServerProviderLocal(resolver, {
      flagClientSecret: CLIENT_SECRET,
      fetch: net.fetch,
    });

    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());
    capturedBodies = [];
  });

  afterEach(async () => {
    await advanceTimersUntil(provider.onClose());
  });

  it('emits telemetry with resolve rate after flagbundle resolve (provider.resolve)', async () => {
    await provider.resolve({ targetingKey: 'tutorial_visitor' }, ['tutorial-feature'], false);

    await advanceTimersUntil(provider.flush());

    expect(capturedBodies.length).toBeGreaterThan(0);

    const allDecoded = capturedBodies.map(body => WriteFlagLogsRequest.decode(body));

    const resolveRates = allDecoded.flatMap(d => d.telemetryData?.resolveRate ?? []);
    const totalResolveCount = resolveRates.reduce((sum, r) => sum + r.count, 0);
    expect(totalResolveCount).toBeGreaterThanOrEqual(1);
  });

  it('emits telemetry with resolve latency after flagbundle resolve (provider.resolve)', async () => {
    await provider.resolve({ targetingKey: 'tutorial_visitor' }, ['tutorial-feature'], false);

    await advanceTimersUntil(provider.flush());

    expect(capturedBodies.length).toBeGreaterThan(0);

    const allDecoded = capturedBodies.map(body => WriteFlagLogsRequest.decode(body));

    const latencyCounts = allDecoded.map(d => d.telemetryData?.resolveLatency?.count ?? 0);
    const totalLatencyCount = latencyCounts.reduce((sum, c) => sum + c, 0);
    expect(totalLatencyCount).toBeGreaterThanOrEqual(1);
  });

  it('emits resolve logs after flagbundle resolve (provider.resolve)', async () => {
    await provider.resolve({ targetingKey: 'tutorial_visitor' }, ['tutorial-feature'], false);

    await advanceTimersUntil(provider.flush());

    expect(capturedBodies.length).toBeGreaterThan(0);

    const allDecoded = capturedBodies.map(body => WriteFlagLogsRequest.decode(body));
    const allFlagResolveInfo = allDecoded.flatMap(d => d.flagResolveInfo);

    expect(allFlagResolveInfo.length).toBeGreaterThanOrEqual(1);
    expect(allFlagResolveInfo[0].length).toBeGreaterThan(0);
  });
});
