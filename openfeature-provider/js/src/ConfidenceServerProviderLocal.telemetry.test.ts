import { beforeEach, expect, it, MockedObject, vi } from 'vitest';
import { LocalResolver } from './LocalResolver';
import { ConfidenceServerProviderLocal } from './ConfidenceServerProviderLocal';
import { advanceTimersUntil, NetworkMock } from './test-helpers';
import { WriteFlagLogsRequest } from './proto/confidence/flags/resolver/v1/internal_api';
import type { EventTracker } from './EventWasmTracker';

vi.mock(import('./hash'), async () => {
  const { sha256Hex } = await import('./test-helpers');
  return { sha256Hex };
});

const EVENTS_URL = 'https://events.confidence.dev/v1/events:publish';

const mockedWasmResolver: MockedObject<LocalResolver> = {
  resolveProcess: vi.fn(),
  registerResolve: vi.fn(),
  setResolverState: vi.fn(),
  flushLogs: vi.fn().mockReturnValue(new Uint8Array(0)),
  flushAssigned: vi.fn().mockReturnValue(new Uint8Array(0)),
  applyFlags: vi.fn(),
  prometheusSnapshot: vi.fn().mockReturnValue(''),
};

/** Yields one event on the first flush, then nothing. */
function singleEventTracker(): EventTracker {
  let remaining = 1;
  return {
    trackEvent() {},
    flushEvents: () => {
      if (remaining-- <= 0) return { events: [] };
      return { events: [{ eventDefinition: 'eventDefinitions/test' }] } as never;
    },
  };
}

/** Reaches the private members the telemetry accounting is observed through. */
type Internals = {
  flushEvents(): Promise<void>;
  flushAssigned(): Promise<void>;
};

vi.useFakeTimers();

let net: NetworkMock;
let sentBodies: Uint8Array[];

beforeEach(() => {
  vi.clearAllMocks();
  vi.clearAllTimers();
  vi.setSystemTime(0);
  net = new NetworkMock();
  sentBodies = [];
  mockedWasmResolver.flushLogs.mockReturnValue(new Uint8Array(0));
  mockedWasmResolver.flushAssigned.mockReturnValue(new Uint8Array(0));
});

/** Captures WriteFlagLogs bodies; `eventsStatus` shapes the events response. */
function makeProvider(eventsResponse: () => Response): ConfidenceServerProviderLocal {
  const fetchImpl: typeof fetch = async (input, init) => {
    const req = new Request(input, init);
    if (req.url === EVENTS_URL) return eventsResponse();
    if (req.url.includes('clientFlagLogs:write')) {
      sentBodies.push(new Uint8Array(await req.arrayBuffer()));
      return new Response(null, { status: 200 });
    }
    return net.fetch(input, init);
  };
  return new ConfidenceServerProviderLocal(mockedWasmResolver, singleEventTracker(), {
    flagClientSecret: 'flagClientSecret',
    fetch: fetchImpl,
  });
}

function decodedTelemetry() {
  return sentBodies.map(b => WriteFlagLogsRequest.decode(b).telemetryData).filter(td => td != null);
}

/**
 * A 200 whose body cannot be decoded is a FAILED batch. Counting success before
 * the decode let the catch also record a failure, so one response produced both
 * a success and a failure.
 */
it('counts an undecodable 200 event response as failed only', async () => {
  const provider = makeProvider(() => new Response(new Uint8Array([0xff, 0xff, 0xff, 0xff]), { status: 200 }));
  // eventTracker is only wired up by initialize(), which this test skips.
  (provider as unknown as { eventTracker: EventTracker }).eventTracker = singleEventTracker();

  await advanceTimersUntil((provider as unknown as Internals).flushEvents());

  // Flush so the drained event counters are stamped onto a request.
  mockedWasmResolver.flushLogs.mockReturnValueOnce(new Uint8Array(100));
  await advanceTimersUntil(provider.flush());

  const events = decodedTelemetry().map(td => td!.events);
  const succeeded = events.reduce((n, e) => n + (e?.batchesSucceeded ?? 0), 0);
  const failed = events.reduce((n, e) => n + (e?.batchesFailed ?? 0), 0);

  expect(failed, 'an undecodable 200 body must count as a failed batch').toBe(1);
  expect(succeeded, 'the same batch must not also be counted as succeeded').toBe(0);
});

/**
 * flushAssigned runs on every evaluation, so if it bypasses the counting path
 * most WriteFlagLogs deliveries are never counted at all.
 */
it('counts assign flushes as WriteFlagLogs deliveries', async () => {
  const provider = makeProvider(() => new Response(null, { status: 200 }));

  // Only the assign path produces a payload, so any flush.succeeded observed
  // can only have come from that delivery.
  mockedWasmResolver.flushAssigned.mockReturnValueOnce(new Uint8Array(50));
  await advanceTimersUntil((provider as unknown as Internals).flushAssigned());

  mockedWasmResolver.flushLogs.mockReturnValueOnce(new Uint8Array(100));
  await advanceTimersUntil(provider.flush());

  const succeeded = decodedTelemetry().reduce((n, td) => n + (td!.flush?.succeeded ?? 0), 0);

  expect(succeeded, 'the assign-flush delivery was never counted').toBeGreaterThanOrEqual(1);
});

/**
 * Event delivery outcomes ride on the next WriteFlagLogs, so onClose must drain
 * events BEFORE the final flush or the last batch's counters never leave the
 * process. Java already orders it this way.
 */
it('drains events before the final log flush on close', async () => {
  const order: string[] = [];
  const fetchImpl: typeof fetch = async (input, init) => {
    const req = new Request(input, init);
    if (req.url === EVENTS_URL) {
      order.push('events');
      return new Response(new Uint8Array(0), { status: 200 });
    }
    if (req.url.includes('clientFlagLogs:write')) {
      order.push('flagLogs');
      return new Response(null, { status: 200 });
    }
    return net.fetch(input, init);
  };
  const provider = new ConfidenceServerProviderLocal(mockedWasmResolver, singleEventTracker(), {
    flagClientSecret: 'flagClientSecret',
    fetch: fetchImpl,
  });
  (provider as unknown as { eventTracker: EventTracker }).eventTracker = singleEventTracker();
  mockedWasmResolver.flushLogs.mockReturnValue(new Uint8Array(100));

  await advanceTimersUntil(provider.onClose());

  const firstEvents = order.indexOf('events');
  const firstFlagLogs = order.indexOf('flagLogs');
  expect(firstEvents, 'events were never published: ' + order.join(',')).toBeGreaterThanOrEqual(0);
  expect(firstFlagLogs, 'flag logs were never sent: ' + order.join(',')).toBeGreaterThanOrEqual(0);
  expect(
    firstEvents,
    'events drained after the final log flush, so their counters are lost: ' + order.join(','),
  ).toBeLessThan(firstFlagLogs);
});
