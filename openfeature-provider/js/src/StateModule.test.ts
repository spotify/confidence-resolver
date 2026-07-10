import { beforeEach, describe, expect, it } from 'vitest';
import { StateModule, LogChunks } from './StateModule';
import { readFileSync } from 'node:fs';
import { ResolveReason, ResolveFlagsRequest, WriteFlagLogsRequest } from './proto/state_module/api';

// The state module wasm is a compiled resolver_state.pb — generate it before running this test
const stateModuleBytes = readFileSync(__dirname + '/../../../wasm/state_module.wasm');
const wasmModule = new WebAssembly.Module(stateModuleBytes);

const RESOLVE_REQUEST: ResolveFlagsRequest = {
  flags: ['flags/tutorial-feature'],
  clientSecret: 'mkjJruAATQWjeY7foFIWfVAcBWnci2YF',
  apply: true,
  evaluationContext: {
    targeting_key: 'tutorial_visitor',
    visitor_id: 'tutorial_visitor',
  },
};

describe('StateModule', () => {
  it('resolves flags', () => {
    const module = new StateModule(wasmModule);
    const response = module.resolveFlags(RESOLVE_REQUEST);
    expect(response.resolvedFlags).toMatchObject([{ reason: ResolveReason.RESOLVE_REASON_MATCH }]);
  });

  describe('onLogs', () => {
    let payloads: LogChunks[];
    let module: StateModule;

    beforeEach(() => {
      payloads = [];
      module = new StateModule(wasmModule, { onLogs: logs => payloads.push(logs) });
    });

    it('delivers chunks that concatenate to a WriteFlagLogsRequest', () => {
      module.resolveFlags(RESOLVE_REQUEST);
      module.flushLogs();

      expect(payloads.length).toBeGreaterThan(0);
      const logs = WriteFlagLogsRequest.decode(payloads[0].copy());
      expect(logs.flagAssigned.length + logs.flagResolveInfo.length).toBeGreaterThan(0);
      payloads.forEach(payload => payload.free());
    });

    it('is re-iterable until freed, minting equal views each time', () => {
      module.resolveFlags(RESOLVE_REQUEST);
      module.flushLogs();

      const payload = payloads[0];
      expect(payload.copy()).toEqual(payload.copy());
      payload.free();
    });

    it('survives module calls between iterations', () => {
      module.resolveFlags(RESOLVE_REQUEST);
      module.flushLogs();

      const payload = payloads[0];
      const before = payload.copy();
      // module calls may grow memory, detaching old views — fresh iteration must still work
      for (let i = 0; i < 100; i++) module.resolveFlags(RESOLVE_REQUEST);
      expect(payload.copy()).toEqual(before);
      payloads.forEach(p => p.free());
    });

    it('free is idempotent and iteration afterwards throws', () => {
      module.resolveFlags(RESOLVE_REQUEST);
      module.flushLogs();

      const payload = payloads[0];
      payload.free();
      payload.free();
      expect(() => [...payload]).toThrow('after free');
    });
  });
});
