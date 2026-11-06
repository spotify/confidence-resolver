/**
 * End-to-end tests against real Confidence infrastructure: the module CDN,
 * the compiled client module, and the flag-log ingestion endpoint.
 *
 * Requires a credential with a published compiled client module:
 *   CONFIDENCE_CLIENT_SECRET, CONFIDENCE_CLIENT_ENCRYPTION_KEY
 * Optional overrides:
 *   CONFIDENCE_PROBE_FLAG (default: material-segment-targed-flag),
 *   CONFIDENCE_PROBE_TARGETING_KEY (default: probe-user)
 *
 * Skips when the credentials are absent, so local runs and PR CI stay green;
 * the `.test_e2e` docker stage provides the credentials from BuildKit secrets.
 */
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';
import { OpenFeature } from '@openfeature/server-sdk';
import { ConfidenceServerProviderLocal } from './provider';

const CLIENT_SECRET = process.env.CONFIDENCE_CLIENT_SECRET;
const ENCRYPTION_KEY = process.env.CONFIDENCE_CLIENT_ENCRYPTION_KEY;
const PROBE_FLAG = process.env.CONFIDENCE_PROBE_FLAG ?? 'material-segment-targed-flag';
const TARGETING_KEY = process.env.CONFIDENCE_PROBE_TARGETING_KEY ?? 'probe-user';

describe.skipIf(!CLIENT_SECRET || !ENCRYPTION_KEY)('ConfidenceServerProviderLocal e2e (real infrastructure)', () => {
  let provider: ConfidenceServerProviderLocal;
  const ingestionResults: Array<{ url: string; status: number }> = [];

  beforeAll(async () => {
    // Observe ingestion deliveries without changing behavior.
    const realFetch = fetch;
    const observingFetch: typeof fetch = (input, init) =>
      realFetch(input, init).then(response => {
        const url = typeof input === 'string' ? input : input instanceof Request ? input.url : String(input);
        if (url.includes('flagLogs:write')) {
          ingestionResults.push({ url, status: response.status });
        }
        return response;
      });
    provider = new ConfidenceServerProviderLocal({
      flagClientSecret: CLIENT_SECRET!,
      encryptionKey: ENCRYPTION_KEY!,
      fetch: observingFetch,
      flushInterval: 1000,
    });
    await OpenFeature.setProviderAndWait('e2e', provider);
  }, 60_000);

  afterAll(async () => {
    await provider.onClose();
    await OpenFeature.close();
  });

  it('initializes to READY with a real module from the CDN', () => {
    expect(provider.status).toBe('READY');
  });

  it('evaluates a real flag through the module', async () => {
    const client = OpenFeature.getClient('e2e');
    const details = await client.getBooleanDetails(`${PROBE_FLAG}.enabled`, false, { targetingKey: TARGETING_KEY });
    // The evaluation must complete through the module — any flag-level
    // outcome (match, default, not-found) is valid, but no provider error.
    expect(['PROVIDER_NOT_READY', 'PROVIDER_FATAL', 'GENERAL']).not.toContain(details.errorCode);
  });

  it('resolves bundles through the module (React path)', async () => {
    const bundle = await provider.resolve({ targetingKey: TARGETING_KEY }, [PROBE_FLAG]);
    expect(typeof bundle.resolveToken).toBe('string');
  });

  it('delivers flushed flag logs to the real ingestion endpoint', async () => {
    await provider.flush();
    await vi.waitFor(() => expect(ingestionResults.length).toBeGreaterThan(0), { timeout: 10_000 });
    expect(ingestionResults.every(result => result.status === 200)).toBe(true);
  });

  it('returns a real Prometheus metrics snapshot from the module', async () => {
    expect((await provider.getPrometheusMetrics()).length).toBeGreaterThan(0);
  });
});
