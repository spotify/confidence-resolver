import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { OpenFeature } from '@openfeature/server-sdk';

const mocks = vi.hoisted(() => ({
  resolverWasmUrl: 'data:application/wasm;base64,resolver',
  eventWasmUrl: 'data:application/wasm;base64,event',
  trackers: [] as Array<{ trackEvent: ReturnType<typeof vi.fn> }>,
}));

vi.mock('../../../wasm/confidence_resolver.wasm', () => ({ default: mocks.resolverWasmUrl }));
vi.mock('../../../wasm/confidence_event_engine.wasm', () => ({ default: mocks.eventWasmUrl }));

vi.mock('./WasmResolver', () => ({
  WasmResolver: class {
    flushLogs(): Uint8Array {
      return new Uint8Array();
    }

    setResolverState(): void {}
  },
}));

vi.mock('./EventWasmTracker', () => ({
  EventWasmTracker: class {
    readonly trackEvent = vi.fn();

    constructor() {
      mocks.trackers.push(this);
    }

    flushEvents() {
      return { events: [] };
    }
  },
}));

describe.each([
  {
    name: 'fetch',
    load: () => import('./index.fetch'),
    resolverWasmUrl: 'https://example.test/confidence_resolver.wasm',
    eventWasmUrl: 'confidence_event_engine.wasm',
  },
  {
    name: 'inlined',
    load: () => import('./index.inlined'),
    resolverWasmUrl: mocks.resolverWasmUrl,
    eventWasmUrl: mocks.eventWasmUrl,
  },
])('$name entry point event tracking', ({ name, load, resolverWasmUrl, eventWasmUrl }) => {
  const fetchMock = vi.fn(async (input: URL | RequestInfo) => {
    const url = String(input);
    if (url.includes('confidence-resolver-state-cdn.spotifycdn.com')) {
      return new Response(new Uint8Array());
    }
    return new Response();
  });

  beforeEach(() => {
    mocks.trackers.length = 0;
    vi.stubGlobal('fetch', fetchMock);
    vi.spyOn(WebAssembly, 'compileStreaming').mockResolvedValue({} as WebAssembly.Module);
  });

  afterEach(async () => {
    await OpenFeature.close();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it('loads the event WASM and tracks through the OpenFeature client', async () => {
    const { createConfidenceServerProvider } = await load();
    const provider = createConfidenceServerProvider({
      flagClientSecret: 'test-secret',
      fetch: fetchMock,
      ...(name === 'fetch' ? { wasmUrl: resolverWasmUrl } : {}),
    });

    await OpenFeature.setProviderAndWait(name, provider);
    OpenFeature.getClient(name).track('checkout_completed', { targetingKey: 'user-123' }, { value: 42 });

    expect(fetchMock.mock.calls.map(([input]) => String(input))).toEqual(
      expect.arrayContaining([expect.stringContaining(resolverWasmUrl), expect.stringContaining(eventWasmUrl)]),
    );
    expect(mocks.trackers).toHaveLength(1);
    expect(mocks.trackers[0].trackEvent).toHaveBeenCalledWith(
      expect.objectContaining({
        eventName: 'checkout_completed',
        value: 42,
        context: { targeting_key: 'user-123' },
      }),
    );
  });
});
