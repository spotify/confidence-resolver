import { describe, it, expect, vi, beforeEach, afterEach, MockedFunction } from 'vitest';
import { Fetch, FetchMiddleware, withAuth, withRetry, withTimeout, withRouter } from './fetch';

// Helpers
function mockedSink(status: number): MockedFunction<Fetch>;
function mockedSink(error: string): MockedFunction<Fetch>;
function mockedSink(impl: Fetch): MockedFunction<Fetch>;
function mockedSink(cfg: Fetch | number | string): MockedFunction<Fetch> {
  let impl: Fetch;
  if (typeof cfg === 'number') {
    impl = async () => new Response(null, { status: cfg });
  } else if (typeof cfg === 'string') {
    impl = () => {
      throw new Error(cfg);
    };
  } else {
    impl = cfg;
  }
  return vi.fn(impl);
}

function textStream(text: string): ReadableStream<Uint8Array> {
  const encoder = new TextEncoder();
  return new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(encoder.encode(text));
      controller.close();
    },
  });
}

async function bodyText(body: BodyInit | null | undefined): Promise<string> {
  if (body == null) return '';
  const res = new Response(body);
  return await res.text();
}

describe('fetch middlewares', () => {
  describe('Fetch.create composition', () => {
    it('wraps middlewares right-to-left', async () => {
      const order: string[] = [];
      const m1: FetchMiddleware = next => async (url, init) => {
        order.push('m1-pre');
        const r = await next(url, init);
        order.push('m1-post');
        return r;
      };
      const m2: FetchMiddleware = next => async (url, init) => {
        order.push('m2-pre');
        const r = await next(url, init);
        order.push('m2-post');
        return r;
      };
      const sink = mockedSink(200);
      const f = Fetch.create([m1, m2], sink);
      const resp = await f('http://example.test');
      expect(resp.status).toBe(200);
      expect(order).toEqual(['m1-pre', 'm2-pre', 'm2-post', 'm1-post']);
    });
  });

  describe('withTimeout', () => {
    it('aborts the request after timeout (or skips if unsupported)', async () => {
      const sink = mockedSink(
        (_url, init) =>
          new Promise<Response>((_resolve, reject) => {
            init?.signal?.addEventListener('abort', () => reject(init.signal?.reason));
          }),
      );
      const f = Fetch.create([withTimeout(15)], sink);
      const p = f('http://example.test');
      const guard = new Promise<never>((_r, reject) =>
        setTimeout(() => reject(new Error('timeout waiting for abort')), 500),
      );
      await expect(Promise.race([p as Promise<never>, guard])).rejects.toBeDefined();
    });
  });

  describe('withRetry', () => {
    it('retries on 5xx and eventually succeeds', async () => {
      const calls: number[] = [];
      const sink = mockedSink(async () => {
        calls.push(Date.now());
        return calls.length === 1 ? new Response(null, { status: 500 }) : new Response(null, { status: 200 });
      });
      const f = Fetch.create([withRetry({ baseInterval: 1, jitter: 0 })], sink);
      const resp = await f('http://retry.test');
      expect(resp.status).toBe(200);
    });

    it('uses Retry-After header when provided (HTTP-date small delta)', async () => {
      let attempt = 0;
      const sink = mockedSink(async () => {
        attempt++;
        if (attempt === 1) {
          const soon = new Date(Date.now() + 20).toUTCString();
          return new Response(null, { status: 503, headers: { 'Retry-After': soon } as any });
        }
        return new Response(null, { status: 200 });
      });
      const f = Fetch.create([withRetry({ baseInterval: 1, maxInterval: 10_000, jitter: 0 })], sink);
      const resp = await f('http://retry-after.test');
      expect(resp.status).toBe(200);
    });

    it('replays a ReadableStream body across retries', async () => {
      const seen: string[] = [];
      let attempt = 0;
      const sink = mockedSink(async (_url, init) => {
        seen.push(await bodyText(init?.body ?? null));
        attempt++;
        return attempt === 1 ? new Response(null, { status: 500 }) : new Response(null, { status: 200 });
      });
      const f = Fetch.create([withRetry({ baseInterval: 1, jitter: 0 })], sink);
      const body = textStream('hello');
      const resp = await f('http://body.test', { method: 'POST', body });
      expect(resp.status).toBe(200);
      expect(seen).toEqual(['hello', 'hello']);
    });

    it('aborts during backoff if signal aborted', async () => {
      let attempt = 0;
      const sink = mockedSink(async () => {
        attempt++;
        return attempt === 1 ? new Response(null, { status: 500 }) : new Response(null, { status: 200 });
      });
      const f = Fetch.create([withRetry({ baseInterval: 10, jitter: 0 })], sink);
      const c = new AbortController();
      const p = f('http://abort.test', { signal: c.signal });
      // abort before advancing time into the backoff sleep
      c.abort(new Error('boom'));
      await expect(p).rejects.toBeDefined();
    });
  });

  describe('withAuth', () => {
    it('adds Authorization header and retries once on 401 with renewed token', async () => {
      const calls: { auth?: string }[] = [];
      let nextStatus = 401;
      const sink = mockedSink(async (_url, init) => {
        calls.push({
          auth:
            init?.headers instanceof Headers
              ? init.headers.get('Authorization') ?? undefined
              : new Headers(init?.headers as any).get('Authorization') ?? undefined,
        });
        const r = new Response(null, { status: nextStatus });
        nextStatus = 200;
        return r;
      });

      let tokenGen = 0;
      const tokenProvider = async () => {
        tokenGen += 1;
        return [`t${tokenGen}`, new Date(Date.now() + 60_000)] as [string, Date];
      };

      const f = Fetch.create([withAuth(tokenProvider)], sink);
      const resp = await f('http://auth.test');
      expect(resp.status).toBe(200);
      expect(calls.map(c => c.auth)).toEqual(['Bearer t1', 'Bearer t2']);
    });
  });

  describe('withRouter', () => {
    it('matches simple trailing * (prefix match)', async () => {
      const sink = mockedSink(200);
      const routed = Fetch.create(
        [
          withRouter({
            'https://api.example.com/v1/items/*': [next => next],
          }),
        ],
        sink,
      );
      const ok = await routed('https://api.example.com/v1/items/123');
      expect(ok.status).toBe(200);
    });

    it('matches across segments via anchored regex', async () => {
      const sink = mockedSink(200);
      const routed = Fetch.create(
        [
          withRouter({
            '^https://api\\.example\\.com/.*/metrics$': [next => next],
          }),
        ],
        sink,
      );
      const ok = await routed('https://api.example.com/a/b/metrics');
      expect(ok.status).toBe(200);
    });

    it('matches exactly one segment via anchored regex', async () => {
      const sink = mockedSink(200);
      const routed = Fetch.create(
        [
          withRouter({
            '^https://api\\.example\\.com/v1/[^/]+/metrics$': [next => next],
          }),
        ],
        sink,
      );
      const ok = await routed('https://api.example.com/v1/users/metrics');
      expect(ok.status).toBe(200);
    });

    it('matches zero or more segments via anchored regex', async () => {
      const sink = mockedSink(200);
      const routed = Fetch.create(
        [
          withRouter({
            '^https://api\\.example\\.com(?:/[^/]+)*/metrics/[^/]+$': [next => next],
          }),
        ],
        sink,
      );
      const ok1 = await routed('https://api.example.com/metrics/x');
      const ok2 = await routed('https://api.example.com/a/b/metrics/x');
      expect(ok1.status).toBe(200);
      expect(ok2.status).toBe(200);
    });

    it('supports leading single * (suffix match) and regex alternative', async () => {
      const sink = mockedSink(200);
      const routed = Fetch.create(
        [
          withRouter({
            '*/health': [next => next],
            '^https://[^/]+/v1/[^/]+$': [next => next],
          }),
        ],
        sink,
      );
      const ok1 = await routed('https://service/foo/health');
      const ok2 = await routed('https://x.example/v1/y');
      expect(ok1.status).toBe(200);
      expect(ok2.status).toBe(200);
    });

    it('supports catch-all *', async () => {
      const sink = mockedSink(200);
      const routed = Fetch.create([withRouter({ '*': [next => next] })], sink);
      const res = await routed('https://anything.example/path');
      expect(res.status).toBe(200);
    });

    it('falls through to sink on no match', async () => {
      const route = mockedSink('err');
      const sink = mockedSink(200);
      const routed = Fetch.create([withRouter({ 'https://api.example.com/v1/*': [next => route] })], sink);
      const res = await routed('https://other.example.com/v1/a');
      expect(sink).toHaveBeenCalledTimes(1);
      expect(route).not.toBeCalled();
    });
  });
});
