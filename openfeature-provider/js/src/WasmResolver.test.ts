import { beforeEach, describe, expect, it, vi } from 'vitest';
import { UnsafeWasmResolver, WasmResolver } from './WasmResolver';
import { readFileSync } from 'node:fs';
import { ResolveProcessRequest } from './proto/confidence/wasm/wasm_api';
import { ResolveReason } from './proto/confidence/flags/resolver/v1/types';
import { WriteFlagLogsRequest } from './proto/test-only';

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

let wasmResolver: WasmResolver;

describe('basic operation', () => {
  beforeEach(() => {
    wasmResolver = new WasmResolver(module);
  });

  it('should fail to resolve without state', () => {
    expect(() => {
      wasmResolver.resolveProcess(RESOLVE_REQUEST);
    }).toThrowError('Resolver state not set');
  });

  describe('with state', () => {
    beforeEach(() => {
      wasmResolver.setResolverState(SET_STATE_REQUEST);
    });

    it('should resolve flags', () => {
      const resp = wasmResolver.resolveProcess(RESOLVE_REQUEST);

      expect(resp).toMatchObject({
        resolved: {
          response: {
            resolvedFlags: [
              {
                reason: ResolveReason.RESOLVE_REASON_MATCH,
              },
            ],
          },
        },
      });
    });

    describe('flushLogs', () => {
      it('should be empty before any resolve', () => {
        const logs = wasmResolver.flushLogs();
        expect(logs.length).toBe(0);
      });

      it('should contain logs after a resolve', () => {
        wasmResolver.resolveProcess(RESOLVE_REQUEST);

        const decoded = WriteFlagLogsRequest.decode(wasmResolver.flushLogs());

        expect(decoded.flagAssigned.length).toBe(1);
        expect(decoded.clientResolveInfo.length).toBe(1);
        expect(decoded.flagResolveInfo.length).toBe(1);
      });
    });
  });
});

describe('panic handling', () => {
  const resolveProcessSpy = vi.spyOn(UnsafeWasmResolver.prototype, 'resolveProcess');
  const setResolverStateSpy = vi.spyOn(UnsafeWasmResolver.prototype, 'setResolverState');

  const throwUnreachable = () => {
    throw new WebAssembly.RuntimeError('unreachable');
  };

  beforeEach(() => {
    vi.resetAllMocks();
    wasmResolver = new WasmResolver(module);
  });

  it('throws and reloads the instance on panic', () => {
    wasmResolver.setResolverState(SET_STATE_REQUEST);
    resolveProcessSpy.mockImplementationOnce(throwUnreachable);

    expect(() => {
      wasmResolver.resolveProcess(RESOLVE_REQUEST);
    }).to.throw('unreachable');

    // now it should succeed since the instance is reloaded
    expect(() => {
      wasmResolver.resolveProcess(RESOLVE_REQUEST);
    }).to.not.throw();
  });

  it('can handle panic in setResolverState', () => {
    setResolverStateSpy.mockImplementation(throwUnreachable);

    expect(() => {
      wasmResolver.setResolverState(SET_STATE_REQUEST);
    }).to.throw('unreachable');

    expect(() => {
      wasmResolver.resolveProcess(RESOLVE_REQUEST);
    }).to.throw('state not set');
  });

  it('tries to extracts logs from panicked instance', () => {
    wasmResolver.setResolverState(SET_STATE_REQUEST);

    // create some logs
    wasmResolver.resolveProcess(RESOLVE_REQUEST);

    resolveProcessSpy.mockImplementationOnce(throwUnreachable);

    expect(() => {
      wasmResolver.resolveProcess(RESOLVE_REQUEST);
    }).to.throw('unreachable');

    const logs = wasmResolver.flushLogs();

    expect(logs.length).toBeGreaterThan(0);
  });
});
