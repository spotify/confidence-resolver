import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { OpenFeature } from '@openfeature/server-sdk';
import { ConfidenceServerProviderLocal } from './ConfidenceServerProviderLocal';
import { readFileSync } from 'node:fs';
import { WasmResolver } from './WasmResolver';
import { LoggerBackend, logger } from './logger';
import { createCapturingLoggingBackend } from './test-helpers';

/**
 * End-to-end tests that verify WriteFlagLogs successfully sends to the real backend.
 */

const moduleBytes = readFileSync(__dirname + '/../../../wasm/confidence_resolver.wasm');
const FLAG_CLIENT_SECRET = 'ti5Sipq5EluCYRG7I5cdbpWC3xq7JTWv';
const TARGETING_KEY = 'test-a';

describe('WriteFlagLogs Backend E2E tests', () => {
  let resolver: WasmResolver;
  let provider: ConfidenceServerProviderLocal;
  let capturingBackend: LoggerBackend & { hasErrorLogs(): boolean; hasAnyLogs(): boolean };

  beforeAll(async () => {
    // Set up log capturing with fresh backend
    capturingBackend = createCapturingLoggingBackend();
    await logger.configure(capturingBackend);

    const module = new WebAssembly.Module(moduleBytes);
    resolver = new WasmResolver(module);
    provider = new ConfidenceServerProviderLocal(resolver, {
      flagClientSecret: FLAG_CLIENT_SECRET,
    });

    await OpenFeature.setProviderAndWait(provider);
    OpenFeature.setContext({
      targetingKey: TARGETING_KEY,
      sticky: false,
    });
  });

  afterAll(() => OpenFeature.close());

  it('should successfully send WriteFlagLogs to real backend with no errors', async () => {
    const client = OpenFeature.getClient();

    // Perform a resolve to generate logs
    const value = await client.getBooleanValue('web-sdk-e2e-flag.bool', true);
    expect(value).toBeFalsy();

    // Wait a bit for async log sending to complete
    await new Promise(resolve => setTimeout(resolve, 1000));

    // Verify that some logs were captured
    const hasAnyLogs = capturingBackend.hasAnyLogs();
    expect(hasAnyLogs).toBe(true);

    // Verify no error logs were captured
    const hasErrors = capturingBackend.hasErrorLogs();
    expect(hasErrors).toBe(false);
  });
});
