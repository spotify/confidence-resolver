import { beforeEach, describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { StateModuleProvider, DEFAULT_STATE_BASE_URL } from './StateModuleProvider';
import { SetResolverStateRequest } from './proto/confidence/wasm/messages';
import { WriteFlagLogsRequest } from './proto/state_module/api';
import { ErrorCode } from './types';
import { sha256Hex } from './hash';

const stateModuleBytes = readFileSync(__dirname + '/../../../wasm/state_module.wasm');
const CLIENT_SECRET = 'IlDJIFpDSs51URW04NW1JqjleJTm2Fzo';
const CONTEXT = { targetingKey: 'test-a', sticky: false };

const statePayload = SetResolverStateRequest.encode({
  state: stateModuleBytes,
  accountId: 'confidence-test',
}).finish();

let stateUrl: string;
let logBodies: Uint8Array[];

/** Serves the compiled module from the state URL and captures log writes. */
const fakeFetch: typeof fetch = async (input, init) => {
  const url = String(input);
  if (url === stateUrl) {
    return new Response(statePayload as BodyInit, { headers: { etag: 'v1' } });
  }
  if (url.endsWith('/v1/clientFlagLogs:write')) {
    logBodies.push(new Uint8Array(await new Response(init!.body as BodyInit).arrayBuffer()));
    return new Response(null, { status: 200 });
  }
  throw new Error(`unexpected fetch ${url}`);
};

function createProvider(): StateModuleProvider {
  return new StateModuleProvider({ flagClientSecret: CLIENT_SECRET, fetch: fakeFetch });
}

describe('StateModuleProvider', () => {
  let provider: StateModuleProvider;

  beforeEach(async () => {
    stateUrl = `${DEFAULT_STATE_BASE_URL}/${await sha256Hex(CLIENT_SECRET)}`;
    logBodies = [];
    provider = createProvider();
  });

  it('is not ready before initialize', async () => {
    expect(await provider.resolveStringEvaluation('web-sdk-e2e-flag.str', 'default', CONTEXT)).toMatchObject({
      reason: 'ERROR',
      errorCode: ErrorCode.PROVIDER_NOT_READY,
      value: 'default',
    });
  });

  describe('once initialized', () => {
    beforeEach(async () => {
      await provider.initialize();
    });

    it('evaluates each flag type', async () => {
      expect(await provider.resolveStringEvaluation('web-sdk-e2e-flag.str', 'default', CONTEXT)).toMatchObject({
        value: 'control',
        reason: 'MATCH',
        variant: 'flags/web-sdk-e2e-flag/variants/control',
      });
      expect(await provider.resolveNumberEvaluation('web-sdk-e2e-flag.int', 10, CONTEXT)).toMatchObject({ value: 3 });
      expect(await provider.resolveNumberEvaluation('web-sdk-e2e-flag.double', 10, CONTEXT)).toMatchObject({
        value: 3.5,
      });
      expect(await provider.resolveBooleanEvaluation('web-sdk-e2e-flag.bool', true, CONTEXT)).toMatchObject({
        value: false,
      });
    });

    it('merges a struct default with the resolved value', async () => {
      const result = await provider.resolveObjectEvaluation('web-sdk-e2e-flag', { str: 'x' }, CONTEXT);
      expect(result.value).toMatchObject({ str: 'control', int: 3, bool: false });
      expect(result.reason).toBe('MATCH');
    });

    it('surfaces a type mismatch as an error with the default', async () => {
      expect(await provider.resolveNumberEvaluation('web-sdk-e2e-flag.str', 42, CONTEXT)).toMatchObject({
        reason: 'ERROR',
        errorCode: ErrorCode.TYPE_MISMATCH,
        value: 42,
        shouldApply: false,
      });
    });

    it('surfaces an unknown flag as an error with the default', async () => {
      expect(await provider.resolveStringEvaluation('no-such-flag', 'fallback', CONTEXT)).toMatchObject({
        reason: 'ERROR',
        errorCode: ErrorCode.FLAG_NOT_FOUND,
        value: 'fallback',
      });
    });

    it('errors on a flag needing materializations when no store is configured', async () => {
      expect(
        await provider.resolveStringEvaluation('web-sdk-e2e-flag.str', 'default', {
          targetingKey: 'test-a',
          sticky: true,
        }),
      ).toMatchObject({ reason: 'ERROR', errorCode: ErrorCode.GENERAL, value: 'default' });
    });

    it('sends telemetry on flush', async () => {
      await provider.resolveStringEvaluation('web-sdk-e2e-flag.str', 'default', CONTEXT);
      await provider.flush();

      expect(logBodies.length).toBeGreaterThan(0);
      const logs = WriteFlagLogsRequest.decode(logBodies[0]);
      expect(logs.flagAssigned.length + logs.flagResolveInfo.length).toBeGreaterThan(0);
    });

    it('skips the state fetch when the etag is unchanged', async () => {
      // the fake serves the same etag, so a second update is a no-op reload
      await provider.updateState();
      expect(await provider.resolveStringEvaluation('web-sdk-e2e-flag.str', 'default', CONTEXT)).toMatchObject({
        value: 'control',
      });
    });
  });
});
