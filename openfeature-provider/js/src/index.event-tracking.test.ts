import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { OpenFeature } from '@openfeature/server-sdk';
import { encryptModuleWrapper, moduleArtifactBytes } from './test-helpers';

const CLIENT_SECRET = 'event-tracking-secret';
const ENCRYPTION_KEY = '00'.repeat(32);
const MODULE_BASE = 'https://confidence-resolver-module-cdn.spotifycdn.com';

const mocks = vi.hoisted(() => ({
  eventWasmUrl: 'data:application/wasm;base64,event',
  trackers: [] as Array<{ trackEvent: ReturnType<typeof vi.fn> }>,
}));

vi.mock('../../../wasm/confidence_event_engine.wasm', () => ({ default: mocks.eventWasmUrl }));

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
  { name: 'fetch', load: () => import('./index.fetch'), eventWasmUrl: 'confidence_event_engine.wasm' },
  { name: 'inlined', load: () => import('./index.inlined'), eventWasmUrl: mocks.eventWasmUrl },
])('$name entry point event tracking', ({ name, load, eventWasmUrl }) => {
  const fetchMock = vi.fn(async (input: URL | RequestInfo) => {
    const url = new URL(input instanceof Request ? input.url : String(input));
    if (url.href.startsWith(`${MODULE_BASE}/modules/`)) {
      return new Response(
        encryptModuleWrapper(moduleArtifactBytes(CLIENT_SECRET), 'header.{}.signature', ENCRYPTION_KEY),
      );
    }
    return new Response(new Uint8Array([0, 97, 115, 109])); // wasm magic for compileStreaming consumers
  });

  beforeEach(() => {
    mocks.trackers.length = 0;
    fetchMock.mockClear();
    vi.stubGlobal('fetch', fetchMock);
    vi.spyOn(WebAssembly, 'compileStreaming').mockResolvedValue({} as WebAssembly.Module);
  });

  afterEach(async () => {
    await OpenFeature.close();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it('rejects missing keys before loading WASM or fetching modules', async () => {
    const { createConfidenceServerProvider } = await load();
    // @ts-expect-error encryptionKey is required, including on factory APIs.
    expect(() => createConfidenceServerProvider({ flagClientSecret: CLIENT_SECRET })).toThrow('64 hexadecimal');
    expect(fetchMock).not.toHaveBeenCalled();
    expect(WebAssembly.compileStreaming).not.toHaveBeenCalled();
  });

  it('loads the event WASM and tracks through the OpenFeature client', async () => {
    const { createConfidenceServerProvider } = await load();
    const provider = createConfidenceServerProvider({
      flagClientSecret: CLIENT_SECRET,
      encryptionKey: ENCRYPTION_KEY,
      fetch: fetchMock,
      ...(name === 'fetch' ? { wasmUrl: 'https://example.test/confidence_resolver.wasm' } : {}),
    });

    await OpenFeature.setProviderAndWait(name, provider);
    OpenFeature.getClient(name).track('checkout_completed', { targetingKey: 'user-123' }, { value: 42 });

    expect(fetchMock.mock.calls.map(([input]) => String(input))).toEqual(
      expect.arrayContaining([
        expect.stringContaining(`${MODULE_BASE}/modules/`),
        expect.stringContaining(eventWasmUrl),
      ]),
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

it('node entry point rejects missing keys before resolving WASM files', async () => {
  const { createConfidenceServerProvider } = await import('./index.node');
  // @ts-expect-error encryptionKey is required.
  expect(() => createConfidenceServerProvider({ flagClientSecret: 'secret' })).toThrow('64 hexadecimal');
});
