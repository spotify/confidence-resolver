import type {
  AnyProviderEvent,
  EventContext,
  EventHandler,
  Logger,
  ProviderEventEmitter,
  ServerProviderEvents,
} from '@openfeature/core';
import { getLogger } from './logger';

/** OpenFeature lifecycle events without a Node runtime dependency. */
export class ProviderEvents implements ProviderEventEmitter<ServerProviderEvents> {
  private readonly handlers = new Map<AnyProviderEvent, EventHandler[]>();
  private logger: Pick<Logger, 'error'> = getLogger('provider-events');

  constructor(private readonly providerName: string) {}

  emit(eventType: ServerProviderEvents, context?: EventContext): void {
    for (const handler of this.getHandlers(eventType)) {
      try {
        Promise.resolve(handler({ ...context, providerName: this.providerName })).catch(error =>
          this.logger.error('Error running event handler:', error),
        );
      } catch (error) {
        this.logger.error('Error running event handler:', error);
      }
    }
  }

  addHandler(eventType: AnyProviderEvent, handler: EventHandler): void {
    this.handlers.set(eventType, [...this.getHandlers(eventType), handler]);
  }

  removeHandler(eventType: AnyProviderEvent, handler: EventHandler): void {
    const handlers = this.handlers.get(eventType);
    const index = handlers?.lastIndexOf(handler) ?? -1;
    if (index !== -1) handlers!.splice(index, 1);
  }

  removeAllHandlers(eventType?: AnyProviderEvent): void {
    if (eventType) this.handlers.delete(eventType);
    else this.handlers.clear();
  }

  getHandlers(eventType: AnyProviderEvent): EventHandler[] {
    return [...(this.handlers.get(eventType) ?? [])];
  }

  setLogger(logger: Logger): this {
    this.logger = logger;
    return this;
  }
}
