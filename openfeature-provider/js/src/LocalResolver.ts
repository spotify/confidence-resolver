import type { ResolveWithStickyRequest, ResolveWithStickyResponse } from './proto/confidence/wasm/wasm_api';
import type { SetResolverStateRequest } from './proto/confidence/wasm/messages';
import type { ApplyFlagsRequest } from './proto/confidence/flags/resolver/v1/api';

export interface LocalResolver {
  resolveWithSticky(request: ResolveWithStickyRequest): ResolveWithStickyResponse;
  setResolverState(request: SetResolverStateRequest): void;
  flushLogs(): Uint8Array;
  flushAssigned(): Uint8Array;
  applyFlags(request: ApplyFlagsRequest): void;
}
