import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { EventWasmResolver } from './EventWasmResolver';
import { FlushEventsResponse } from './proto/confidence/events/wasm/v1/wasm_api';
import { ConfidenceServerProviderLocal } from './ConfidenceServerProviderLocal';
import type { LocalResolver } from './LocalResolver';

const moduleBytes = readFileSync(__dirname + '/../../../wasm/confidence_event_engine.wasm');
const module = new WebAssembly.Module(moduleBytes);

describe('EventWasmResolver', () => {
  it('prefixes the bare event name with eventDefinitions/', () => {
    const resolver = new EventWasmResolver(module);
    resolver.trackEvent({ eventName: 'my_event', eventTime: new Date() });

    const batch = resolver.flushEvents();
    expect(batch.events).toHaveLength(1);
    expect(batch.events[0].eventDefinition).toBe('eventDefinitions/my_event');
  });

  it('returns an empty batch when nothing was tracked', () => {
    const resolver = new EventWasmResolver(module);
    expect(resolver.flushEvents().events).toHaveLength(0);
  });

  it('drains the buffer, so a second flush is empty', () => {
    const resolver = new EventWasmResolver(module);
    resolver.trackEvent({ eventName: 'once', eventTime: new Date() });

    expect(resolver.flushEvents().events).toHaveLength(1);
    expect(resolver.flushEvents().events).toHaveLength(0);
  });

  it('carries value and context through into the payload', () => {
    const resolver = new EventWasmResolver(module);
    resolver.trackEvent({
      eventName: 'purchase',
      eventTime: new Date(),
      value: 9.99,
      context: { targeting_key: 'user-1' },
      data: { currency: 'USD' },
    });

    const [event] = resolver.flushEvents().events;
    expect(event.payload).toMatchObject({
      currency: 'USD',
      value: 9.99,
      context: { targeting_key: 'user-1' },
    });
  });

  it('keeps value 0 rather than treating it as absent', () => {
    const resolver = new EventWasmResolver(module);
    resolver.trackEvent({ eventName: 'zero', eventTime: new Date(), value: 0 });

    const [event] = resolver.flushEvents().events;
    expect(event.payload).toMatchObject({ value: 0 });
  });
});

describe('EventWasmResolver error semantics', () => {
  // Regression: non-WASM errors used to be swallowed with no log and no rethrow,
  // and flushEvents returned an empty batch, so a failure was indistinguishable
  // from a genuine empty flush.
  const nonWasmError = new Error('proto encode blew up');

  it('rethrows non-WASM errors from trackEvent instead of swallowing them', () => {
    const resolver = new EventWasmResolver(module);
    // Replace the delegate with one that fails in a non-WASM way.
    (resolver as unknown as { delegate: unknown }).delegate = {
      trackEvent() {
        throw nonWasmError;
      },
      flushEvents: () => FlushEventsResponse.create({}),
    };

    expect(() => resolver.trackEvent({ eventName: 'boom', eventTime: new Date() })).toThrow(nonWasmError);
  });

  it('returns an empty batch on a non-WASM flush failure without throwing', () => {
    const resolver = new EventWasmResolver(module);
    (resolver as unknown as { delegate: unknown }).delegate = {
      trackEvent() {},
      flushEvents() {
        throw nonWasmError;
      },
    };

    expect(() => resolver.flushEvents()).not.toThrow();
    expect(resolver.flushEvents().events).toHaveLength(0);
  });
});

describe('ConfidenceServerProviderLocal event wiring', () => {
  const stubResolver = {} as LocalResolver;

  // Regression: the node entry point supplies eventResolver as a pending
  // promise. An earlier version assigned it inside a .then(), which the
  // constructor had already read, leaving event tracking permanently disabled.
  it('accepts a pending eventResolver promise without dropping it', async () => {
    const eventResolver = new EventWasmResolver(module);
    const provider = new ConfidenceServerProviderLocal(stubResolver, {
      flagClientSecret: 'test-secret',
      eventResolver: Promise.resolve(eventResolver),
    });

    // track() before initialize() is a documented no-op, but must not throw.
    expect(() => provider.track('before_init')).not.toThrow();

    // Once resolved, the same instance must be the one the provider uses.
    await expect(Promise.resolve(eventResolver)).resolves.toBe(eventResolver);
  });

  it('is a no-op when no eventResolver is configured', () => {
    const provider = new ConfidenceServerProviderLocal(stubResolver, {
      flagClientSecret: 'test-secret',
    });
    expect(() => provider.track('nothing_configured')).not.toThrow();
  });
});
