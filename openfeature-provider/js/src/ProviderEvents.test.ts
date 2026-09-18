import { expect, it, vi } from 'vitest';
import { ProviderEvents as Events } from '@openfeature/server-sdk';
import { ProviderEvents } from './ProviderEvents';

it('adds and removes lifecycle handlers, including duplicate registrations', () => {
  const events = new ProviderEvents('test-provider');
  const handler = vi.fn();
  events.addHandler(Events.Ready, handler);
  events.addHandler(Events.Ready, handler);
  events.removeHandler(Events.Ready, handler);
  events.emit(Events.Ready);
  expect(handler).toHaveBeenCalledTimes(1);
  expect(handler).toHaveBeenCalledWith({ providerName: 'test-provider' });
  events.removeAllHandlers(Events.Ready);
  expect(events.getHandlers(Events.Ready)).toEqual([]);
  events.addHandler(Events.Error, handler);
  events.removeAllHandlers();
  expect(events.getHandlers(Events.Error)).toEqual([]);
});

it('isolates failing handlers so recovery notifications reach other listeners', async () => {
  const events = new ProviderEvents('test-provider');
  const logger = { error: vi.fn(), warn: vi.fn(), info: vi.fn(), debug: vi.fn() };
  events.setLogger(logger);
  events.addHandler(Events.Ready, () => {
    throw new Error('sync handler failure');
  });
  events.addHandler(Events.Ready, async () => {
    throw new Error('async handler failure');
  });
  const handler = vi.fn();
  events.addHandler(Events.Ready, handler);
  events.emit(Events.Ready);
  await Promise.resolve();
  expect(handler).toHaveBeenCalledTimes(1);
  expect(logger.error).toHaveBeenCalledTimes(2);
});
