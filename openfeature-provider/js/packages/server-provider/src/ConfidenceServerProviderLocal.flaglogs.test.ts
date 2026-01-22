import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';
import { OpenFeature } from '@openfeature/server-sdk';
import { ConfidenceServerProviderLocal } from './ConfidenceServerProviderLocal';
import { readFileSync } from 'node:fs';
import { WasmResolver } from './WasmResolver';
import { WriteFlagLogsRequest } from './proto/test-only';

/**
 * Unit tests that verify WriteFlagLogs contains correct flag assignment data.
 *
 * These tests use the WasmResolver.flushLogs() method to capture flag log data
 * and verify that flag assignment data is captured correctly.
 *
 * Note: The JS proto represents flagAssigned, clientResolveInfo, and flagResolveInfo
 * as arrays of raw bytes (Uint8Array). The tests verify that data is captured,
 * though full parsing of the nested messages would require additional proto definitions.
 */

const moduleBytes = readFileSync(__dirname + '/../../../../../wasm/confidence_resolver.wasm');
const FLAG_CLIENT_SECRET = 'ti5Sipq5EluCYRG7I5cdbpWC3xq7JTWv';
const TARGETING_KEY = 'test-a';

describe('WriteFlagLogs tests', () => {
  let resolver: WasmResolver;
  let provider: ConfidenceServerProviderLocal;

  beforeAll(async () => {
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

  beforeEach(() => {
    // Flush any existing logs before each test
    resolver.flushLogs();
  });

  afterAll(() => OpenFeature.close());

  it('should capture WriteFlagLogs after boolean resolve', async () => {
    const client = OpenFeature.getClient();

    // Resolve a boolean flag
    const value = await client.getBooleanValue('web-sdk-e2e-flag.bool', true);
    expect(value).toBeFalsy();

    // Get and decode the captured logs
    const logsBytes = resolver.flushLogs();
    expect(logsBytes.length).toBeGreaterThan(0);

    const decoded = WriteFlagLogsRequest.decode(logsBytes);

    // Verify flag_assigned entries are captured
    expect(decoded.flagAssigned.length).toBeGreaterThanOrEqual(1);

    // Each flagAssigned entry is raw bytes - verify they're non-empty
    expect(decoded.flagAssigned[0].length).toBeGreaterThan(0);
  });

  it('should capture correct counts after string resolve', async () => {
    const client = OpenFeature.getClient();

    // Resolve a string flag
    const value = await client.getStringValue('web-sdk-e2e-flag.str', 'default');
    expect(value).toEqual('control');

    // Get and decode the captured logs
    const logsBytes = resolver.flushLogs();
    expect(logsBytes.length).toBeGreaterThan(0);

    const decoded = WriteFlagLogsRequest.decode(logsBytes);

    // Verify all log types are captured
    expect(decoded.flagAssigned.length).toBeGreaterThanOrEqual(1);
    expect(decoded.clientResolveInfo.length).toBeGreaterThanOrEqual(1);
    expect(decoded.flagResolveInfo.length).toBeGreaterThanOrEqual(1);
  });

  it('should capture client resolve info', async () => {
    const client = OpenFeature.getClient();

    // Perform a resolve
    await client.getNumberValue('web-sdk-e2e-flag.int', 10);

    // Get and decode the captured logs
    const logsBytes = resolver.flushLogs();
    const decoded = WriteFlagLogsRequest.decode(logsBytes);

    // Verify client_resolve_info is captured
    expect(decoded.clientResolveInfo.length).toBeGreaterThanOrEqual(1);
    // Each entry is raw bytes - verify they're non-empty
    expect(decoded.clientResolveInfo[0].length).toBeGreaterThan(0);
  });

  it('should capture flag resolve info', async () => {
    const client = OpenFeature.getClient();

    // Perform a resolve
    await client.getNumberValue('web-sdk-e2e-flag.double', 10.0);

    // Get and decode the captured logs
    const logsBytes = resolver.flushLogs();
    const decoded = WriteFlagLogsRequest.decode(logsBytes);

    // Verify flag_resolve_info is captured
    expect(decoded.flagResolveInfo.length).toBeGreaterThanOrEqual(1);
    // Each entry is raw bytes - verify they're non-empty
    expect(decoded.flagResolveInfo[0].length).toBeGreaterThan(0);
  });

  it('should capture multiple resolves', async () => {
    const client = OpenFeature.getClient();

    // Perform multiple resolves
    await client.getBooleanValue('web-sdk-e2e-flag.bool', true);
    await client.getStringValue('web-sdk-e2e-flag.str', 'default');
    await client.getNumberValue('web-sdk-e2e-flag.int', 10);
    await client.getNumberValue('web-sdk-e2e-flag.double', 10.0);

    // Get and decode the captured logs
    const logsBytes = resolver.flushLogs();
    const decoded = WriteFlagLogsRequest.decode(logsBytes);

    // Should have captured log entries for all resolves
    expect(decoded.flagAssigned.length).toBeGreaterThanOrEqual(4);
  });

  it('should capture non-empty flag assigned data', async () => {
    const client = OpenFeature.getClient();

    // Perform a resolve
    await client.getBooleanValue('web-sdk-e2e-flag.bool', true);

    // Get and decode the captured logs
    const logsBytes = resolver.flushLogs();
    const decoded = WriteFlagLogsRequest.decode(logsBytes);

    expect(decoded.flagAssigned.length).toBeGreaterThanOrEqual(1);

    // Verify the flagAssigned bytes contain data (resolve_id, flags, etc.)
    // We can't easily parse the nested proto, but we can verify it's substantial
    const flagAssignedBytes = decoded.flagAssigned[0];
    expect(flagAssignedBytes.length).toBeGreaterThan(10); // Should have meaningful data
  });

  it('should clear logs after flush', async () => {
    const client = OpenFeature.getClient();

    // Perform a resolve
    await client.getBooleanValue('web-sdk-e2e-flag.bool', true);

    // First flush should have logs
    const firstFlush = resolver.flushLogs();
    expect(firstFlush.length).toBeGreaterThan(0);

    // Second flush should be empty (logs were cleared)
    const secondFlush = resolver.flushLogs();
    expect(secondFlush.length).toBe(0);
  });

  it('should capture telemetry data', async () => {
    const client = OpenFeature.getClient();

    // Perform a resolve
    await client.getBooleanValue('web-sdk-e2e-flag.bool', true);

    // Get and decode the captured logs
    const logsBytes = resolver.flushLogs();
    const decoded = WriteFlagLogsRequest.decode(logsBytes);

    // Telemetry data should be present (contains SDK info)
    expect(decoded.telemetryData.length).toBeGreaterThan(0);
  });
});
