import { describe, it, expect } from 'vitest';
import { UnsafeWasmResolver } from './WasmResolver';
import { readFileSync } from 'node:fs';
import { ResolveProcessRequest } from './proto/confidence/wasm/wasm_api';

const moduleBytes = readFileSync(__dirname + '/../../../wasm/confidence_resolver.wasm');
const stateBytes = readFileSync(__dirname + '/../../../wasm/resolver_state.pb');

const module = new WebAssembly.Module(moduleBytes);
const CLIENT_SECRET = 'mkjJruAATQWjeY7foFIWfVAcBWnci2YF';

const RESOLVE_REQUEST: ResolveProcessRequest = {
  deferredMaterializations: {
    flags: ['flags/tutorial-feature'],
    clientSecret: CLIENT_SECRET,
    apply: true,
    evaluationContext: {
      targeting_key: 'tutorial_visitor',
      visitor_id: 'tutorial_visitor',
    },
  },
};

const SET_STATE_REQUEST = { state: stateBytes, accountId: 'confidence-test' };

describe('wasm memory stability', () => {
  it('should not leak memory on repeated resolve calls', () => {
    const resolver = new UnsafeWasmResolver(module);
    resolver.setResolverState(SET_STATE_REQUEST);

    // Warm up to settle one-time allocations
    for (let i = 0; i < 50_000; i++) {
      resolver.resolveProcess(RESOLVE_REQUEST);
      if (i % 1000 === 0) resolver.flushLogs();
    }

    const memBefore = (resolver as any).exports.memory.buffer.byteLength;

    for (let i = 0; i < 50_000; i++) {
      resolver.resolveProcess(RESOLVE_REQUEST);
      if (i % 1000 === 0) resolver.flushLogs();
    }

    const memAfter = (resolver as any).exports.memory.buffer.byteLength;

    expect(memAfter).toBe(memBefore);
  });
});
