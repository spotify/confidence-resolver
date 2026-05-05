import { OpenFeature } from '@openfeature/server-sdk';
import type { NextApiRequest, NextApiResponse } from 'next';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { applyHandler } from './api';
import { __resetKeyCacheForTests, sealResolveToken } from './token';

function makeReqRes(
  method: string,
  body?: unknown,
): { req: NextApiRequest; res: NextApiResponse; status: () => number } {
  const headers: Record<string, string> = {};
  let statusCode = 200;
  const res = {
    setHeader: vi.fn((k: string, v: string) => {
      headers[k] = v;
    }),
    status: vi.fn((code: number) => {
      statusCode = code;
      return res;
    }),
    end: vi.fn(),
  } as unknown as NextApiResponse;
  const req = { method, body } as NextApiRequest;
  return { req, res, status: () => statusCode };
}

describe('applyHandler', () => {
  beforeAll(() => {
    process.env.CONFIDENCE_TOKEN_KEY = 'test-key-do-not-use-in-prod';
    __resetKeyCacheForTests();
  });

  afterEach(() => {
    OpenFeature.clearProviders();
  });

  it('returns 405 for non-POST', async () => {
    const handler = applyHandler();
    const { req, res, status } = makeReqRes('GET');
    await handler(req, res);
    expect(status()).toBe(405);
    expect(res.setHeader).toHaveBeenCalledWith('Allow', 'POST');
  });

  it('returns 400 when body is missing', async () => {
    const handler = applyHandler();
    const { req, res, status } = makeReqRes('POST');
    await handler(req, res);
    expect(status()).toBe(400);
  });

  it('returns 400 when fields have wrong types', async () => {
    const handler = applyHandler();
    const { req, res, status } = makeReqRes('POST', { resolveToken: 123, flagName: 'x' });
    await handler(req, res);
    expect(status()).toBe(400);
  });

  it('returns 503 when no Confidence provider is registered', async () => {
    const handler = applyHandler();
    const sealed = sealResolveToken('opaque');
    const { req, res, status } = makeReqRes('POST', { resolveToken: sealed, flagName: 'my-flag' });
    await handler(req, res);
    expect(status()).toBe(503);
  });

  it('returns 400 when the resolveToken cannot be opened', async () => {
    // Register a real-shaped provider so the 503 branch doesn't shortcut.
    OpenFeature.setProvider({
      metadata: { name: 'ConfidenceServerProviderLocal' },
      applyFlag: vi.fn(),
    } as never);
    const handler = applyHandler();
    const { req, res, status } = makeReqRes('POST', { resolveToken: 'not-a-real-handle', flagName: 'x' });
    await handler(req, res);
    expect(status()).toBe(400);
  });

  it('calls applyFlag on success and returns 204', async () => {
    const applyFlag = vi.fn();
    OpenFeature.setProvider({
      metadata: { name: 'ConfidenceServerProviderLocal' },
      applyFlag,
    } as never);
    const sealed = sealResolveToken('the-real-token');
    const handler = applyHandler();
    const { req, res, status } = makeReqRes('POST', { resolveToken: sealed, flagName: 'my-flag' });
    await handler(req, res);
    expect(applyFlag).toHaveBeenCalledWith('the-real-token', 'my-flag');
    expect(status()).toBe(204);
  });
});
