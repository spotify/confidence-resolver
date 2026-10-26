import { afterEach, describe, expect, it } from 'vitest';
import { ConfidenceServerProviderLocal } from './ConfidenceServerProviderLocal';
import { noopEventTracker, encryptTestState, TEST_ENCRYPTION_KEY } from './test-helpers';
import type { LocalResolver } from './LocalResolver';
import type {
  ResolveProcessRequest,
  ResolveProcessResponse,
  RegisterResolveRequest,
} from './proto/confidence/wasm/wasm_api';
import type { SetResolverStateRequest } from './proto/confidence/wasm/messages';
import type { ApplyFlagsRequest } from './proto/confidence/flags/resolver/v1/api';
import {
  WriteFlagLogsRequest,
  IngestFlagLogsRequest,
  FlagResolveInfo,
} from './proto/confidence/flags/resolver/v1/internal_api';
import { ClientResolverState, LogDestination } from './proto/confidence/flags/admin/v1/resolver';
import { SdkId } from './proto/confidence/flags/resolver/v1/types';

/**
 * Regression tests for the resolve-report upload round trip.
 *
 * The WASM resolver writes FlagResolveInfo with both variant_resolve_info and
 * rule_resolve_info (per-assignment resolve counts). Before rule_resolve_info
 * was declared in the provider proto, `enrichTelemetry`'s decode→encode pass
 * silently dropped the undeclared field: ts-proto discards unknown fields, so
 * the backend never received per-treatment resolve counts and the
 * "Flag resolved" traffic check failed with zeros while the flag-level
 * variant totals kept rising.
 *
 * These tests feed resolver-shaped bytes through the provider's real flush
 * path (enrichTelemetry → sendFlagLogs → fetch) and assert the payload that
 * actually goes on the wire still carries both groups' counts — for the
 * Spotify edge destination and for the Cloudflare ingest wrap, which adds a
 * second decode→encode hop.
 *
 * No WASM or network is needed: the only behaviour under test is what the
 * host does to the bytes between the resolver and the wire.
 */

const FLAG = 'flags/js-provider-roundtrip-flag';
const RULE = 'flags/js-provider-roundtrip-flag/rules/js-provider-roundtrip-rule';
const TREATMENT_1 = 'workflows/abtest/instances/roundtrip-demo/treatments/cohort-1';
const TREATMENT_2 = 'workflows/abtest/instances/roundtrip-demo/treatments/cohort-2';

/** Assert one flag's resolve report kept both variant totals and both
 *  treatments' assignment counts — everything the backend needs to store
 *  per-treatment resolveCount. */
function expectFlagReportPreserved(flagInfo: FlagResolveInfo): void {
  expect(flagInfo.flag).toBe(FLAG);

  // Variant totals survive the round trip
  expect(flagInfo.variantResolveInfo.map(v => [v.variant, v.count])).toEqual([
    [`${FLAG}/variants/cohort-1`, 3],
    [`${FLAG}/variants/cohort-2`, 2],
  ]);

  // Per-rule and per-assignment resolve counts survive the round trip —
  // these are what the backend stores as each treatment's resolveCount.
  expect(flagInfo.ruleResolveInfo).toHaveLength(1);
  const ruleInfo = flagInfo.ruleResolveInfo[0];
  expect(ruleInfo.rule).toBe(RULE);
  expect(ruleInfo.count).toBe(5);
  expect(ruleInfo.assignmentResolveInfo.map(a => [a.assignmentId, a.count])).toEqual([
    [TREATMENT_1, 3],
    [TREATMENT_2, 2],
  ]);
}

/** A resolver stand-in whose flush output is exactly what the real WASM
 *  resolver serializes for a two-treatment experiment: variant totals plus
 *  rule and per-assignment resolve counts. */
class FakeReportResolver implements LocalResolver {
  private pendingLogs: Uint8Array[] = [];

  queueReport(variant1Count: number, variant2Count: number): void {
    this.pendingLogs.push(
      WriteFlagLogsRequest.encode({
        flagAssigned: [],
        clientResolveInfo: [],
        flagResolveInfo: [
          {
            flag: FLAG,
            variantResolveInfo: [
              { variant: `${FLAG}/variants/cohort-1`, count: variant1Count },
              { variant: `${FLAG}/variants/cohort-2`, count: variant2Count },
            ],
            ruleResolveInfo: [
              {
                rule: RULE,
                count: variant1Count + variant2Count,
                assignmentResolveInfo: [
                  { assignmentId: TREATMENT_1, count: variant1Count },
                  { assignmentId: TREATMENT_2, count: variant2Count },
                ],
              },
            ],
          },
        ],
      }).finish(),
    );
  }

  resolveProcess(_request: ResolveProcessRequest): ResolveProcessResponse {
    throw new Error('resolveProcess is not used by these tests');
  }
  registerResolve(_request: RegisterResolveRequest): void {}
  setResolverState(_request: SetResolverStateRequest): void {}
  applyFlags(_request: ApplyFlagsRequest): void {}
  flushLogs(): Uint8Array {
    return this.pendingLogs.shift() ?? new Uint8Array(0);
  }
  flushAssigned(): Uint8Array {
    return new Uint8Array(0);
  }
  prometheusSnapshot(_instance: string): string {
    return '';
  }
}

describe('rule resolve info upload round trip', () => {
  let provider: ConfidenceServerProviderLocal | undefined;

  afterEach(async () => {
    if (provider) {
      await provider.onClose();
      provider = undefined;
    }
  });

  it('preserves per-treatment resolve counts when re-encoding a flush for the Spotify edge destination', async () => {
    const edgeBodies: Uint8Array[] = [];
    const resolver = new FakeReportResolver();
    provider = new ConfidenceServerProviderLocal(resolver, noopEventTracker, {
      flagClientSecret: 'test-secret',
      encryptionKey: TEST_ENCRYPTION_KEY,
      fetch: async (input, init) => {
        const req = new Request(input, init);
        const url = new URL(req.url);
        if (url.hostname === 'resolver.confidence.dev' && url.pathname === '/v1/clientFlagLogs:write') {
          edgeBodies.push(new Uint8Array(await req.arrayBuffer()));
          return new Response(null, { status: 200 });
        }
        return new Response(null, { status: 404 });
      },
    });

    // First flush: enrichTelemetry decodes and re-encodes to stamp provider
    // init telemetry — the re-encode that used to drop rule_resolve_info.
    resolver.queueReport(3, 2);
    await provider.flush();
    expect(edgeBodies).toHaveLength(1);

    const first = WriteFlagLogsRequest.decode(edgeBodies[0]);
    expectFlagReportPreserved(first.flagResolveInfo[0]);

    // The init stamp proves the body was re-encoded, not forwarded verbatim.
    expect(first.telemetryData?.sdk?.id).toBe(SdkId.SDK_ID_JS_LOCAL_SERVER_PROVIDER);

    // Second flush: re-encoded to attach delivery counters instead.
    resolver.queueReport(3, 2);
    await provider.flush();
    expect(edgeBodies).toHaveLength(2);

    const second = WriteFlagLogsRequest.decode(edgeBodies[1]);
    expectFlagReportPreserved(second.flagResolveInfo[0]);
    expect(second.telemetryData?.flush?.succeeded).toBeGreaterThanOrEqual(1);
  });

  it('preserves per-treatment resolve counts through the Cloudflare ingest wrap', async () => {
    const ingestBodies: Uint8Array[] = [];
    let edgeCalls = 0;
    const resolver = new FakeReportResolver();
    provider = new ConfidenceServerProviderLocal(resolver, noopEventTracker, {
      flagClientSecret: 'test-secret',
      encryptionKey: TEST_ENCRYPTION_KEY,
      fetch: async (input, init) => {
        const req = new Request(input, init);
        const url = new URL(req.url);
        if (url.hostname === 'epx-flags-logs.experimentation-platform.workers.dev') {
          ingestBodies.push(new Uint8Array(await req.arrayBuffer()));
          return new Response(null, { status: 200 });
        }
        if (url.hostname === 'confidence-resolver-state-cdn.spotifycdn.com') {
          return new Response(
            encryptTestState(
              ClientResolverState.encode({
                state: new Uint8Array(0),
                account: 'roundtrip-account',
                logDestinations: [LogDestination.LOG_DESTINATION_CLOUDFLARE],
              }).finish(),
            ),
          );
        }
        if (url.hostname === 'resolver.confidence.dev') {
          edgeCalls++;
          return new Response(null, { status: 200 });
        }
        return new Response(null, { status: 404 });
      },
    });

    // The CDN state routes flag logs to the Cloudflare ingest destination.
    await provider.initialize();

    resolver.queueReport(3, 2);
    await provider.flush();

    expect(ingestBodies).toHaveLength(1);
    expect(edgeCalls).toBe(0);

    // The Cloudflare path adds a second decode→encode hop: the batch is
    // decoded and re-wrapped into IngestFlagLogsRequest before upload.
    const ingest = IngestFlagLogsRequest.decode(ingestBodies[0]);
    expect(ingest.accountId).toBe('roundtrip-account');
    expectFlagReportPreserved(ingest.batch!.flagResolveInfo[0]);
  });
});
