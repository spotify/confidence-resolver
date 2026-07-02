import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { OpenFeature } from '@openfeature/server-sdk';
import { ConfidenceServerProviderLocal } from './ConfidenceServerProviderLocal';
import { readFileSync } from 'node:fs';
import { WasmResolver } from './WasmResolver';

const moduleBytes = readFileSync(__dirname + '/../../../wasm/confidence_resolver.wasm');
const module = new WebAssembly.Module(moduleBytes);

describe.each([
  { name: 'unencrypted', encryptionKey: undefined },
  { name: 'encrypted', encryptionKey: process.env.CONFIDENCE_CLIENT_ENCRYPTION_KEY },
])('ConfidenceServerProvider E2E ($name)', ({ name, encryptionKey }) => {
  const resolver = new WasmResolver(module);
  const provider = new ConfidenceServerProviderLocal(resolver, {
    flagClientSecret: process.env.CONFIDENCE_CLIENT_SECRET!,
    encryptionKey,
  });

  beforeAll(async () => {
    await OpenFeature.setProviderAndWait(`e2e-${name}`, provider);
  });

  afterAll(() => OpenFeature.close());

  const ctx = { targetingKey: 'test-a', sticky: false };

  it('should resolve a boolean e2e', async () => {
    const client = OpenFeature.getClient(`e2e-${name}`);

    expect(await client.getBooleanValue('web-sdk-e2e-flag.bool', true, ctx)).toBeFalsy();
  });

  it('should resolve an int', async () => {
    const client = OpenFeature.getClient(`e2e-${name}`);

    expect(await client.getNumberValue('web-sdk-e2e-flag.int', 10, ctx)).toEqual(3);
  });

  it('should resolve a double', async () => {
    const client = OpenFeature.getClient(`e2e-${name}`);

    expect(await client.getNumberValue('web-sdk-e2e-flag.double', 10, ctx)).toEqual(3.5);
  });

  it('should resolve a string', async () => {
    const client = OpenFeature.getClient(`e2e-${name}`);

    expect(await client.getStringValue('web-sdk-e2e-flag.str', 'default', ctx)).toEqual('control');
  });

  it('should resolve a struct', async () => {
    const client = OpenFeature.getClient(`e2e-${name}`);
    const expectedObject = {
      int: 4,
      str: 'obj control',
      bool: false,
      double: 3.6,
      ['obj-obj']: {},
    };

    expect(await client.getObjectValue('web-sdk-e2e-flag.obj', {}, ctx)).toEqual(expectedObject);
  });

  it('should resolve a sub value from a struct', async () => {
    const client = OpenFeature.getClient(`e2e-${name}`);

    expect(await client.getBooleanValue('web-sdk-e2e-flag.obj.bool', true, ctx)).toBeFalsy();
  });

  it('should resolve a sub value from a struct with details with resolve token for client side apply call', async () => {
    const client = OpenFeature.getClient(`e2e-${name}`);
    const expectedObject = {
      flagKey: 'web-sdk-e2e-flag.obj.double',
      reason: 'MATCH',
      variant: 'flags/web-sdk-e2e-flag/variants/control',
      flagMetadata: {},
      value: 3.6,
      shouldApply: true,
    };

    expect(await client.getNumberDetails('web-sdk-e2e-flag.obj.double', 1, ctx)).toEqual(expectedObject);
  });
});

describe('ConfidenceServerProvider E2E (sticky)', () => {
  const resolver = new WasmResolver(module);
  const provider = new ConfidenceServerProviderLocal(resolver, {
    flagClientSecret: process.env.CONFIDENCE_CLIENT_SECRET!,
    materializationStore: 'CONFIDENCE_REMOTE_STORE',
  });

  beforeAll(async () => {
    await OpenFeature.setProviderAndWait('e2e-sticky', provider);
    OpenFeature.setContext({
      targetingKey: 'test-a',
      sticky: false,
    });
  });

  afterAll(() => OpenFeature.close());

  it('should resolve a flag with a sticky resolve', async () => {
    const client = OpenFeature.getClient('e2e-sticky');
    const result = await client.getNumberDetails('web-sdk-e2e-flag.double', -1, {
      targetingKey: 'test-3',
      sticky: true,
    });

    // The flag has a running experiment with a sticky assignment. The intake is paused but we should still get the sticky assignment.
    // If this test breaks it could mean that the experiment was removed or that the bigtable materialization was cleaned out.
    // To restore: open the experiment in the Confidence UI and resume the intake, then run this test once (the resolve re-seeds the sticky materialization for 'test-a'), then pause the intake again.
    expect(result.value).toBe(99.99);
    expect(result.variant).toBe('flags/web-sdk-e2e-flag/variants/sticky');
    expect(result.reason).toBe('MATCH');
  });
});
