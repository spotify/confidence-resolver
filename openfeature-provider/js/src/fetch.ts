import { logger as rootLogger, Logger } from './logger';
import { portableSetTimeout, abortableSleep, abortablePromise } from './util';

const logger = rootLogger.getLogger('fetch');

export type Fetch = (url: string, init?: RequestInit) => Promise<Response>;

export namespace Fetch {
  export function create(middleware: FetchMiddleware[], sink: Fetch = fetch): Fetch {
    return middleware.reduceRight((next, middleware) => middleware(next), sink);
  }
}

export type FetchMiddleware = (next: Fetch) => Fetch;

export namespace FetchMiddleware {
  export function compose(outer: FetchMiddleware, inner: FetchMiddleware): FetchMiddleware {
    return next => outer(inner(next));
  }
}

export function withTimeout(timeoutMs: number): FetchMiddleware {
  return next =>
    (url, init = {}) => {
      const signal = timeoutSignal(timeoutMs, init.signal);
      return next(url, { ...init, signal });
    };
}

export function withStallTimeout(stallTimeoutMs: number): FetchMiddleware {
  return next =>
    async (url, init = {}) => {
      const ac = new AbortController();
      const signal = init.signal ? AbortSignal.any([ac.signal, init.signal]) : ac.signal;
      const abort = () => {
        ac.abort(new Error(`The operation timed out after not receiving data for ${stallTimeoutMs}ms`));
      };
      let timeoutId = portableSetTimeout(abort, stallTimeoutMs);

      const resp = await next(url, { ...init, signal });
      clearTimeout(timeoutId);
      if (resp.body) {
        const chunks: Uint8Array<ArrayBuffer>[] = [];
        timeoutId = portableSetTimeout(abort, stallTimeoutMs);

        for await (const chunk of resp.body) {
          chunks.push(chunk);
          clearTimeout(timeoutId);
          timeoutId = portableSetTimeout(abort, stallTimeoutMs);
        }

        return new Response(new Blob(chunks), {
          status: resp.status,
          statusText: resp.statusText,
          headers: resp.headers,
        });
      }
      return resp;
    };
}

export function withRetry(opts?: {
  maxAttempts?: number;
  baseInterval?: number;
  maxInterval?: number;
  abortAtInterval?: boolean;
  backoff?: number;
  jitter?: number;
}): FetchMiddleware {
  const maxAttempts = opts?.maxAttempts ?? 6;
  const baseInterval = opts?.baseInterval ?? 250;
  const maxInterval = opts?.maxInterval ?? 30_000;
  const backoff = opts?.backoff ?? 2;
  const jitter = opts?.jitter ?? (__TEST__ ? 0 : 0.1);

  return next =>
    async (url, { body, signal, ...init } = {}) => {
      const cloneBody = await bodyRepeater(body);
      let attempts = 0;
      let deadline = 0;

      const calculateDeadline = (): number => {
        const jitterFactor = 1 + 2 * Math.random() * jitter - jitter;
        const delay = jitterFactor * Math.min(maxInterval, baseInterval * Math.pow(backoff, attempts));
        return Date.now() + delay;
      };
      const onSuccess = async (resp: Response) => {
        const { status, statusText } = resp;
        if ((status !== 408 && status !== 429 && status < 500) || attempts >= maxAttempts) {
          return resp;
        }
        logger.debug('withRetry %s failed attempt %d with %d %s', url, attempts - 1, status, statusText);
        const serverDelay = parseRetryAfter(resp.headers.get('Retry-After'), baseInterval, maxInterval);

        await abortableSleep(serverDelay ?? deadline - Date.now(), signal);
        return doTry();
      };
      const onError = async (error: unknown) => {
        logger.debug('withRetry %s failed attempt %d with %s', url, attempts - 1, error);
        if (signal?.aborted || attempts >= maxAttempts) {
          throw error;
        }
        await abortableSleep(deadline - Date.now(), signal);
        return doTry();
      };
      const doTry = (): Promise<Response> => {
        let attemptSignal = signal;
        deadline = calculateDeadline();
        attempts++;
        if (opts?.abortAtInterval) {
          attemptSignal = timeoutSignal(deadline - Date.now(), signal);
        }
        return next(url, { body: cloneBody(), signal: attemptSignal, ...init }).then(onSuccess, onError);
      };

      return doTry();
    };
}

export function withAuth(
  tokenProvider: () => Promise<[token: string, expiry?: Date]>,
  signal?: AbortSignal,
): FetchMiddleware {
  let renewTimeout = 0;
  let current: Promise<string> | null = null;

  signal?.addEventListener('abort', () => {
    clearTimeout(renewTimeout);
  });

  const renewToken = () => {
    logger.debug('withAuth renewing token');
    clearTimeout(renewTimeout);
    current = tokenProvider()
      .then(([token, expiry]) => {
        logger.debug('withAuth renew success %s', expiry && expiry.valueOf() - Date.now());
        if (expiry) {
          const ttl = expiry.valueOf() - Date.now();
          renewTimeout = portableSetTimeout(renewToken, 0.8 * ttl);
        }
        return token;
      })
      .catch(e => {
        current = null;
        throw e;
      });
  };

  const fetchWithToken = async (fetch: Fetch, url: string, init: RequestInit) => {
    const token = await abortablePromise(current!, init.signal);
    const headers = new Headers(init.headers);
    headers.set('Authorization', `Bearer ${token}`);
    return fetch(url, { ...init, headers });
  };

  return next =>
    async (url, init = {}) => {
      const bodyClone = await bodyRepeater(init.body);
      if (!current) {
        renewToken();
      }
      const currentBeforeFetch = current;
      let resp = await fetchWithToken(next, url, { ...init, body: bodyClone() });
      if (resp.status === 401) {
        // there might be a race of multiple simultaneous 401
        if (current === currentBeforeFetch) {
          renewToken();
        }
        // do one quick retry on 401
        resp = await fetchWithToken(next, url, { ...init, body: bodyClone() });
      }
      return resp;
    };
}

export function withRouter(routes: Record<string, FetchMiddleware[]>): FetchMiddleware {
  // Simplified DSL over full URL string:
  // - Anchored regex when pattern starts with ^ and ends with $
  // - '*' alone => match all
  // - Leading single '*' => suffix match
  // - Trailing single '*' => prefix match
  // - Otherwise exact match (any other '*' usage is unsupported)
  const hasOnlyOneStar = (s: string) => s.split('*').length - 1 === 1;
  const compile = (pattern: string): ((url: string) => boolean) => {
    if (pattern.length >= 2 && pattern[0] === '^' && pattern[pattern.length - 1] === '$') {
      const rx = new RegExp(pattern);
      return url => rx.test(url);
    }
    if (pattern === '*') {
      return _ => true;
    }
    if (pattern.includes('|')) {
      const predicates = pattern.split('|').map(compile);
      return url => predicates.some(pred => pred(url));
    }
    if (pattern.startsWith('*') && hasOnlyOneStar(pattern)) {
      const suffix = pattern.slice(1);
      return url => url.endsWith(suffix);
    }
    if (pattern.endsWith('*') && hasOnlyOneStar(pattern)) {
      const prefix = pattern.slice(0, -1);
      return url => url.startsWith(prefix);
    }
    if (pattern.includes('*')) {
      throw new Error(
        `withRouter unsupported pattern "${pattern}". Only single leading or trailing * (or * alone) supported.`,
      );
    }
    return url => url === pattern;
  };
  const preCompiled = Object.entries(routes).map(
    ([pattern, middlewares]): [(url: string) => boolean, FetchMiddleware[]] => {
      const predicate = compile(pattern);
      return [predicate, middlewares];
    },
  );

  return next => {
    const table = preCompiled.map(([predicate, middlewares]): [(url: string) => boolean, Fetch] => {
      const fetch = Fetch.create(middlewares, next);
      return [predicate, fetch];
    });

    return async (url, init = {}) => {
      const match = table.find(([pred]) => pred(url));
      if (!match) {
        logger.info('withRouter no route matched %s, falling through', url);
        return next(url, init);
      }
      return match[1](url, init);
    };
  };
}

export function withResponse(factory: (url: string, init?: RequestInit) => Promise<Response>): FetchMiddleware {
  return _next => factory;
}

const fetchLogger = logger;
export function withLogging(logger: Logger = fetchLogger): FetchMiddleware {
  return next => async (url, init) => {
    const start = Date.now();
    const resp = await next(url, init);
    const duration = Date.now() - start;
    logger.info('%s %s (%i) %dms', (init?.method ?? 'get').toUpperCase(), url.split('?', 1)[0], resp.status, duration);
    return resp;
  };
}

async function bodyRepeater<T extends BodyInit | null | undefined>(body: T): Promise<() => T> {
  if (body instanceof ReadableStream) {
    // TODO this case could be made a little more efficient by body.tee,
    // but we don't use ReadableStreams, so low prio
    const blob = await new Response(body).blob();
    return () => blob.stream() as T;
  }
  return () => body;
}

function parseRetryAfter(retryAfterValue: string | null, min = 0, max = Number.MAX_SAFE_INTEGER): number | undefined {
  if (retryAfterValue) {
    let delay = Number(retryAfterValue) * 1000;
    if (Number.isNaN(delay)) {
      delay = Date.parse(retryAfterValue) - Date.now();
    }
    if (Number.isFinite(delay) && delay > 0) {
      return Math.max(min, Math.min(delay, max));
    }
  }
  return undefined;
}

async function consumeReadableStream(
  stream: ReadableStream,
  onData: (chunk: Uint8Array<ArrayBuffer>) => void,
  onEnd?: (err?: unknown) => void,
): Promise<void> {
  try {
    for await (const chunk of stream) {
      onData(chunk);
    }
    onEnd?.();
  } catch (e: unknown) {
    onEnd?.(e);
  }
}

function timeoutSignal(delay: number, signal?: AbortSignal | null): AbortSignal {
  const ac = new AbortController();
  portableSetTimeout(() => ac.abort(new Error(`Operation timed out after ${delay}ms`)), delay);
  return signal ? AbortSignal.any([signal, ac.signal]) : ac.signal;
}
