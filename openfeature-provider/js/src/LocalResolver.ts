import type { ResolveWithStickyRequest, ResolveWithStickyResponse } from './proto/resolver/api';
import type { SetResolverStateRequest } from './proto/messages';

export interface LocalResolver {
  resolveWithSticky(request: ResolveWithStickyRequest): ResolveWithStickyResponse;
  setResolverState(request: SetResolverStateRequest): void;
  flushLogs(): Uint8Array;
  flushAssigned(): Uint8Array;
}
