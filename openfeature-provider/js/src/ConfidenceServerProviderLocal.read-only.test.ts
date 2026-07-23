import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { WasmResolver } from './WasmResolver';
import { ConfidenceServerProviderLocal } from './ConfidenceServerProviderLocal';
import { advanceTimersUntil, NetworkMock } from './test-helpers';
import { ClientResolverState } from './proto/confidence/flags/admin/v1/resolver';

vi.mock(import('./hash'), async () => {
  const { sha256Hex } = await import('./test-helpers');
  return { sha256Hex };
});

const moduleBytes = readFileSync(__dirname + '/../../../wasm/confidence_resolver.wasm');
const stateBytes = readFileSync(__dirname + '/../../../wasm/resolver_state.pb');
const CLIENT_SECRET = 'mkjJruAATQWjeY7foFIWfVAcBWnci2YF';

vi.useFakeTimers();

describe('read-only mode', () => {
  let net: NetworkMock;
  let flagLogRequests: number;

  function createProvider(readOnly: boolean): ConfidenceServerProviderLocal {
    const module = new WebAssembly.Module(moduleBytes);
    const resolver = new WasmResolver(module);
    return new ConfidenceServerProviderLocal(resolver, {
      flagClientSecret: CLIENT_SECRET,
      fetch: net.fetch,
      readOnly,
    });
  }

  beforeEach(async () => {
    vi.clearAllMocks();
    vi.clearAllTimers();
    vi.setSystemTime(0);

    net = new NetworkMock();
    flagLogRequests = 0;

    net.cdn.state.handler = () =>
      new Response(
        ClientResolverState.encode({
          state: stateBytes,
          account: 'confidence-test',
        }).finish(),
      );

    net.resolver.flagLogs.handler = async () => {
      flagLogRequests += 1;
      return new Response(null, { status: 200 });
    };
  });

  it('resolves flag values without sending any flag logs, telemetry, or apply', async () => {
    const provider = createProvider(true);
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());
    flagLogRequests = 0;

    const bundle = await provider.resolve({ targetingKey: 'tutorial_visitor' }, ['tutorial-feature'], true);
    expect(bundle).toBeDefined();

    await provider.evaluate('tutorial-feature.enabled', false, { targetingKey: 'tutorial_visitor' });

    await advanceTimersUntil(provider.flush());
    await advanceTimersUntil(provider.onClose());

    expect(flagLogRequests).toBe(0);
  });

  it('applyFlag is a no-op in read-only mode', async () => {
    const provider = createProvider(true);
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());
    flagLogRequests = 0;

    const bundle = await provider.resolve({ targetingKey: 'tutorial_visitor' }, ['tutorial-feature'], false);
    provider.applyFlag(bundle.resolveToken, 'tutorial-feature');

    await advanceTimersUntil(provider.flush());
    await advanceTimersUntil(provider.onClose());

    expect(flagLogRequests).toBe(0);
  });

  it('still sends flag logs when read-only is disabled (sanity)', async () => {
    const provider = createProvider(false);
    await advanceTimersUntil(expect(provider.initialize()).resolves.toBeUndefined());
    flagLogRequests = 0;

    await provider.resolve({ targetingKey: 'tutorial_visitor' }, ['tutorial-feature'], false);
    await advanceTimersUntil(provider.flush());
    await advanceTimersUntil(provider.onClose());

    expect(flagLogRequests).toBeGreaterThan(0);
  });
});
