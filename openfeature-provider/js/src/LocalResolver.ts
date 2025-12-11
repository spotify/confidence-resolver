import type { ResolveWithStickyRequest, ResolveWithStickyResponse } from './proto/confidence/wasm/wasm_api';
import type { SetResolverStateRequest } from './proto/confidence/wasm/messages';

export interface LocalResolver {
  resolveWithSticky(request: ResolveWithStickyRequest): ResolveWithStickyResponse;
  setResolverState(request: SetResolverStateRequest): void;
  flushLogs(): Uint8Array;
  flushAssigned(): Uint8Array;
}
