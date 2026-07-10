import { beforeEach, describe, expect, it } from 'vitest';
import { WasmStateResolver } from './WasmStateResolver';
import { readFileSync } from 'node:fs';
import { ResolveProcessRequest } from './proto/confidence/wasm/wasm_api';
import { ResolveReason } from './proto/confidence/flags/resolver/v1/types';

// The state module wasm is a compiled resolver_state.pb — generate it before running this test
const stateModuleBytes = readFileSync(__dirname + '/../../../wasm/state_module.wasm');

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

const SET_STATE_REQUEST = { state: stateModuleBytes, accountId: 'confidence-test' };

let resolver: WasmStateResolver;

describe('WasmStateResolver', () => {
  beforeEach(() => {
    resolver = new WasmStateResolver();
  });

  it('should fail to resolve without state', () => {
    expect(() => {
      resolver.resolveProcess(RESOLVE_REQUEST);
    }).toThrow();
  });

  describe('with state', () => {
    beforeEach(() => {
      resolver.setResolverState(SET_STATE_REQUEST);
    });

    it('should resolve flags', () => {
      const resp = resolver.resolveProcess(RESOLVE_REQUEST);

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

    it('should resolve flags without materializations', () => {
      const request: ResolveProcessRequest = {
        withoutMaterializations: {
          flags: ['flags/tutorial-feature'],
          clientSecret: CLIENT_SECRET,
          apply: true,
          evaluationContext: {
            targeting_key: 'tutorial_visitor',
            visitor_id: 'tutorial_visitor',
          },
        },
      };

      const resp = resolver.resolveProcess(request);

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

    it('should handle flushLogs without crashing', () => {
      resolver.resolveProcess(RESOLVE_REQUEST);
      const logs = resolver.flushLogs();
      expect(logs).toBeInstanceOf(Uint8Array);
    });

    it('should accept a new state module', () => {
      resolver.setResolverState(SET_STATE_REQUEST);
      const resp = resolver.resolveProcess(RESOLVE_REQUEST);
      expect(resp.resolved).toBeDefined();
    });
  });
});
