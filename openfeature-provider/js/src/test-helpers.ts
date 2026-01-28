import { vi } from 'vitest';
import { abortableSleep, isObject, TimeUnit } from './util';
import { ReadableStream as NodeReadableStream } from 'node:stream/web';
import { SetResolverStateRequest } from './proto/confidence/wasm/messages';

type PayloadFactory = (req: Request) => BodyInit | null;
type ByteStream = ReadableStream<Uint8Array<ArrayBuffer>>;

type RequestRecord = {
  startTime: number;
  url: string;
  method: string;
  status: number | string;
};

type HandlerFn = (req: Request) => Response | Promise<Response>;

type StatusProvider = (req: Request) => number | string;

class RequestHandler {
  readonly requests: RequestRecord[] = [];
  latency: number = 0;
  bandwidth: number = Infinity;
  error?: string;

  constructor(public handler: HandlerFn) {}

  get calls(): number {
    return this.requests.length;
  }

  async handle(req: Request): Promise<Response> {
    const startTime = Date.now();
    let status: number | string = 'UNKNOWN';
    try {
      await abortableSleep(this.latency / 2, req.signal);
      if (this.error) {
        throw Object.assign(new Error(this.error), { name: this.error });
      }
      if (req.body && Number.isFinite(this.bandwidth)) {
        req = new Request(req, { body: throttleStream(req.body, this.latency, req.signal) });
      }
      let resp = await this.handler(req);
      status = resp.status;
      await abortableSleep(this.latency / 2, req.signal);
      if (resp.body && Number.isFinite(this.bandwidth)) {
        resp = new Response(throttleStream(resp.body, this.bandwidth, req.signal), {
          status: resp.status,
          statusText: resp.statusText,
          headers: resp.headers,
        });
      }
      return resp;
    } catch (err) {
      status = stringifyErrorType(err);
      throw err;
    } finally {
      this.requests.push({
        startTime,
        url: req.url,
        method: req.method,
        status,
      });
    }
  }
  clear(): void {
    this.requests.length = 0;
  }
}

class RequestDispatcher<T extends RequestHandler> extends RequestHandler {
  constructor(private readonly keyFn: (req: Request) => string, private readonly dispatchMap: Record<string, T>) {
    super(req => {
      const key = this.keyFn(req);
      const handler = this.dispatchMap[key] ?? this.dispatchMap['*'];
      return handler ? handler.handle(req) : new Response(null, { status: 404 });
    });
  }

  clear(): void {
    super.clear();
    return Object.values(this.dispatchMap).forEach(h => h.clear());
  }
}
class EndpointMock extends RequestHandler {
  status: number | string | StatusProvider = 200;

  constructor(private payloadFactory: PayloadFactory = () => null) {
    super(req => {
      let status = this.status;
      if (typeof status === 'function') {
        status = status(req);
      }
      if (typeof status === 'string') {
        throw new Error(status);
      }
      if (status === 200) {
        return new Response(this.payloadFactory(req));
      }
      return new Response(null, { status });
    });
  }
}

class ServerMock extends RequestDispatcher<EndpointMock> {
  constructor(private endpoints: Record<string, EndpointMock>) {
    super(req => new URL(req.url).pathname, endpoints);
  }
}

class ResolverServerMock extends ServerMock {
  readonly flagLogs: EndpointMock;
  readonly readMaterializations: EndpointMock;
  readonly writeMaterializations: EndpointMock;

  constructor() {
    const flagLogs = new EndpointMock();
    const readMaterializations = new EndpointMock();
    const writeMaterializations = new EndpointMock();
    super({
      '/v1/clientFlagLogs:write': flagLogs,
      '/v1/materialization:readMaterializedOperations': readMaterializations,
      '/v1/materialization:writeMaterializedOperations': writeMaterializations,
    });
    this.flagLogs = flagLogs;
    this.readMaterializations = readMaterializations;
    this.writeMaterializations = writeMaterializations;
  }
}

class CdnServerMock extends ServerMock {
  readonly state: EndpointMock;
  constructor() {
    const state = new EndpointMock(() =>
      SetResolverStateRequest.encode({
        state: new Uint8Array(100), // Empty state for testing
        accountId: '<account>',
      }).finish(),
    );
    // CDN serves state at any path (using client secret as path)
    super({
      '*': state,
    });
    this.state = state;
  }
}

export class NetworkMock extends RequestDispatcher<ServerMock> {
  readonly resolver: ResolverServerMock;
  readonly cdn: CdnServerMock;

  constructor() {
    const resolver = new ResolverServerMock();
    const cdn = new CdnServerMock();

    super(req => new URL(req.url).hostname, {
      'resolver.confidence.dev': resolver,
      'confidence-resolver-state-cdn.spotifycdn.com': cdn,
    });
    this.resolver = resolver;
    this.cdn = cdn;
  }

  readonly fetch: typeof fetch = (input, init) => this.handle(new Request(input, init));
}

function throttleStream(stream: ByteStream, bandwidth: number, signal?: AbortSignal): ByteStream {
  const iter = (async function* () {
    for await (const chunk of stream) {
      await abortableSleep((chunk.length / bandwidth) * 1000, signal);
      yield chunk;
    }
  })();
  return NodeReadableStream.from(iter) as ByteStream;
}

function stringifyErrorType(err: unknown): string {
  if (isObject(err) && 'name' in err && typeof err.name === 'string') {
    return err.name;
  }
  return String(err);
}

if (vi.isFakeTimers()) {
  throw new Error('FakeTimers should not be on when test-helpers.ts is loaded!');
}
const realSetImmediate = setImmediate;

export async function advanceTimersUntil(predicate: () => boolean): Promise<void>;
export async function advanceTimersUntil(opt: { timeout?: number }, predicate: () => boolean): Promise<void>;
export async function advanceTimersUntil<T>(promise: Promise<T>): Promise<T>;
export async function advanceTimersUntil<T>(opt: { timeout: number }, promise: Promise<T>): Promise<T>;
export async function advanceTimersUntil(...args: any[]): Promise<any> {
  if (!vi.isFakeTimers()) {
    throw new Error('FakeTimers are not enabled');
  }
  const opt: { timeout?: number } = args.length == 2 ? args.shift() : {};

  let predicate: () => boolean;
  let ret = undefined;
  if (typeof args[0] === 'function') {
    predicate = args[0];
  } else {
    let done = false;
    ret = args[0];
    ret.finally(() => {
      done = true;
    });
    predicate = () => done;
  }

  if (opt.timeout) {
    let timedOut = false;
    const timeout = setTimeout(() => {
      timedOut = true;
    }, opt.timeout);
    const origPred = predicate;
    predicate = () => {
      if (timedOut) {
        throw new Error('advanceTimersUntil: Timed out');
      }
      try {
        if (origPred()) {
          clearTimeout(timeout);
          return true;
        }
        return false;
      } catch (err) {
        clearTimeout(timeout);
        throw err;
      }
    };
  }

  await new Promise(resolve => {
    realSetImmediate(resolve);
  });

  while (!predicate()) {
    // some code, notably NodeJS WHATWG streams and fetch impl. might schedule immediate calls
    // that isn't mocked by fake timers, so we advance that first.
    if (process.getActiveResourcesInfo().includes('Immediate')) {
      await new Promise(resolve => {
        realSetImmediate(resolve);
      });
      continue;
    }
    if (vi.getTimerCount() === 0) {
      throw new Error('advanceTimersUntil: Condition not met and no timers left to advance');
    }
    await vi.advanceTimersToNextTimerAsync();
  }
  return ret;
}

export function createCapturingLoggingBackend() {
  const logs: Array<{ namespace: string; message: string; args: any[] }> = [];

  const backend = (namespace: string) => {
    const logFn: any = (message: string, ...args: any[]) => {
      logs.push({ namespace, message, args });
    };
    logFn.enabled = true;
    return logFn;
  };

  backend.hasErrorLogs = () => logs.some(log => log.namespace.includes(':error'));
  backend.hasAnyLogs = () => logs.length > 0;
  return backend;
}

export async function sha256Hex(input: string): Promise<string> {
  // Simple deterministic hash for testing - just convert string to hex
  // This avoids the async crypto.subtle.digest that doesn't play well with fake timers
  let hash = 0;
  for (let i = 0; i < input.length; i++) {
    const char = input.charCodeAt(i);
    hash = (hash << 5) - hash + char;
    hash = hash & hash; // Convert to 32bit integer
  }
  // Pad to 64 hex chars (SHA-256 size)
  return Math.abs(hash).toString(16).padStart(64, '0');
}
