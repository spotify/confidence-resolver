import { beforeEach, describe, expect, it } from 'vitest';
import { StateModule, LogChunks } from './StateModule';
import { readFileSync } from 'node:fs';
import { ErrorCode, ResolveReason, ResolveFlagsRequest, WriteFlagLogsRequest } from './proto/state_module/api';

// The state module wasm is a compiled resolver_state.pb — generate it before running this test
const stateModuleBytes = readFileSync(__dirname + '/../../../wasm/state_module.wasm');
const wasmModule = new WebAssembly.Module(stateModuleBytes);

const RESOLVE_REQUEST: ResolveFlagsRequest = {
  flags: ['flags/web-sdk-e2e-flag'],
  clientSecret: 'IlDJIFpDSs51URW04NW1JqjleJTm2Fzo',
  apply: true,
  evaluationContext: {
    targeting_key: 'test-a',
    sticky: false,
  },
};

describe('StateModule', () => {
  it('resolves flags', () => {
    const module = new StateModule(wasmModule);
    const handle = module.resolve(RESOLVE_REQUEST);
    expect(handle.decode().resolved?.resolvedFlags).toMatchObject([{ reason: ResolveReason.RESOLVE_REASON_MATCH }]);
    handle.close();
  });

  it('rejects use after close', () => {
    const module = new StateModule(wasmModule);
    const handle = module.resolve(RESOLVE_REQUEST);
    handle.close();
    handle.close();
    expect(() => handle.decode()).toThrow('after close');
  });

  describe('evaluate', () => {
    let module: StateModule;

    beforeEach(() => {
      module = new StateModule(wasmModule);
    });

    it('evaluates several keys against one borrowed response', () => {
      const handle = module.resolve(RESOLVE_REQUEST);
      try {
        expect(handle.evaluate({ flagKey: 'web-sdk-e2e-flag.str', defaultValue: 'default' })).toMatchObject({
          value: 'control',
          reason: ResolveReason.RESOLVE_REASON_MATCH,
          errorCode: ErrorCode.ERROR_CODE_UNSPECIFIED,
        });
        expect(handle.evaluate({ flagKey: 'web-sdk-e2e-flag.int', defaultValue: 10 })).toMatchObject({ value: 3 });
        expect(handle.evaluate({ flagKey: 'web-sdk-e2e-flag.bool', defaultValue: true })).toMatchObject({
          value: false,
        });
      } finally {
        handle.close();
      }
    });

    it('returns the whole flag value with no path', () => {
      const handle = module.resolve(RESOLVE_REQUEST);
      try {
        const details = handle.evaluate({ flagKey: 'web-sdk-e2e-flag', defaultValue: {} });
        expect(details.value).toMatchObject({ str: 'control', int: 3, bool: false });
        expect(details.variant).toMatch(/^flags\/web-sdk-e2e-flag\/variants\//);
      } finally {
        handle.close();
      }
    });

    it('reports a type mismatch in band', () => {
      const handle = module.resolve(RESOLVE_REQUEST);
      try {
        expect(handle.evaluate({ flagKey: 'web-sdk-e2e-flag.str', defaultValue: 42 })).toMatchObject({
          value: 42,
          errorCode: ErrorCode.ERROR_CODE_TYPE_MISMATCH,
          reason: ResolveReason.RESOLVE_REASON_ERROR,
          shouldApply: false,
        });
      } finally {
        handle.close();
      }
    });

    it('reports an unknown flag in band', () => {
      const handle = module.resolve({ ...RESOLVE_REQUEST, flags: ['flags/no-such-flag'] });
      try {
        expect(handle.evaluate({ flagKey: 'no-such-flag', defaultValue: 'fallback' })).toMatchObject({
          value: 'fallback',
          errorCode: ErrorCode.ERROR_CODE_FLAG_NOT_FOUND,
        });
      } finally {
        handle.close();
      }
    });

    it('works against a resumed response', () => {
      // sticky targeting sends the flag through a materialization read
      const started = module.resolveStart({
        ...RESOLVE_REQUEST,
        evaluationContext: { targeting_key: 'test-a', sticky: true },
      });
      const suspended = started.decode().suspended;
      started.close();
      expect(suspended).toBeDefined();

      // no records available: the flag falls back rather than sticking
      const resumed = module.resolveResume(suspended!.processId, []);
      try {
        expect(resumed.decode().resolved).toBeDefined();
        expect(resumed.evaluate({ flagKey: 'web-sdk-e2e-flag.str', defaultValue: 'default' })).toMatchObject({
          errorCode: ErrorCode.ERROR_CODE_UNSPECIFIED,
        });
      } finally {
        resumed.close();
      }
    });
  });

  describe('onLogs', () => {
    let payloads: LogChunks[];
    let module: StateModule;

    beforeEach(() => {
      payloads = [];
      module = new StateModule(wasmModule, { onLogs: logs => payloads.push(logs) });
    });

    function resolveOnce(): void {
      module.resolve(RESOLVE_REQUEST).close();
    }

    it('delivers chunks that concatenate to a WriteFlagLogsRequest', () => {
      resolveOnce();
      module.flushLogs();

      expect(payloads.length).toBeGreaterThan(0);
      const logs = WriteFlagLogsRequest.decode(payloads[0].copy());
      expect(logs.flagAssigned.length + logs.flagResolveInfo.length).toBeGreaterThan(0);
      payloads.forEach(payload => payload.free());
    });

    it('is re-iterable until freed, minting equal views each time', () => {
      resolveOnce();
      module.flushLogs();

      const payload = payloads[0];
      expect(payload.copy()).toEqual(payload.copy());
      payload.free();
    });

    it('survives module calls between iterations', () => {
      resolveOnce();
      module.flushLogs();

      const payload = payloads[0];
      const before = payload.copy();
      // module calls may grow memory, detaching old views — fresh iteration must still work
      for (let i = 0; i < 100; i++) resolveOnce();
      expect(payload.copy()).toEqual(before);
      payloads.forEach(p => p.free());
    });

    it('free is idempotent and iteration afterwards throws', () => {
      resolveOnce();
      module.flushLogs();

      const payload = payloads[0];
      payload.free();
      payload.free();
      expect(() => [...payload]).toThrow('after free');
    });
  });
});
