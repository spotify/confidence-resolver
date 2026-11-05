import { createCipheriv, createHash } from 'node:crypto';
import { describe, expect, it } from 'vitest';
import { ClientModule } from '../proto/confidence/flags/wasm/v1/client_module';
import { ResolverMetadata } from '../proto/confidence/flags/wasm/v1/metadata';
import { ModuleClient } from './module-client';
import { customSection, hostMemoryImport, importSection, wasm } from './test-support';
import { bytesEqual } from '../util';

const CLIENT_SECRET = 'test-secret';
const SECRET_SHA256_HEX = createHash('sha256').update(CLIENT_SECRET, 'utf8').digest('hex');
const ENCRYPTION_KEY = '00'.repeat(32);
const BASE_URL = 'http://cdn.test';

function digestsOf(...secrets: string[]): Uint8Array[] {
  return secrets.map(secret => new Uint8Array(createHash('sha256').update(secret, 'utf8').digest()));
}

/** A byte-level artifact: wasm bytes carrying a conforming confidence.metadata section. */
function artifact(listedSecrets: string[] = [CLIENT_SECRET], variant = 1): Uint8Array {
  const metadata = ResolverMetadata.encode({
    schemaVersion: 1,
    compilerVersion: '1.2.3',
    compilerGitSha: 'deadbeef',
    clientSecretSha256: digestsOf(...listedSecrets),
  }).finish();
  return wasm(customSection('confidence.metadata', metadata), importSection(hostMemoryImport(variant, variant + 1)));
}

/** Encrypt a ClientModule wrapper in the wire format: nonce || ciphertext || tag. */
function encryptedResponse(moduleBytes: Uint8Array, logToken: string, etag: string, status = 200): Response {
  const wrapper = ClientModule.encode({ module: moduleBytes, logToken }).finish();
  const nonce = Buffer.alloc(12);
  const cipher = createCipheriv('aes-256-gcm', Buffer.from(ENCRYPTION_KEY, 'hex'), nonce);
  const body = new Uint8Array(Buffer.concat([nonce, cipher.update(wrapper), cipher.final(), cipher.getAuthTag()]));
  return new Response(body, { status, headers: { etag } });
}

type MockFetch = typeof fetch & {
  requests: Array<{ url: string; ifNoneMatch: string | null }>;
};

function mockFetch(handler: (call: number) => Response | Promise<Response>): MockFetch {
  const requests: MockFetch['requests'] = [];
  let call = 0;
  const impl = ((url: string, init?: RequestInit) => {
    requests.push({ url, ifNoneMatch: new Headers(init?.headers).get('if-none-match') });
    if (init?.signal?.aborted) return Promise.reject(new Error('This operation was aborted'));
    return Promise.resolve(handler(call++));
  }) as MockFetch;
  impl.requests = requests;
  return impl;
}

function createClient(fetchImpl: typeof fetch) {
  return new ModuleClient({
    clientSecret: CLIENT_SECRET,
    encryptionKey: ENCRYPTION_KEY,
    baseUrl: BASE_URL,
    fetch: fetchImpl,
  });
}

describe('ModuleClient construction', () => {
  it('validates the encryption key and client secret', () => {
    expect(
      () =>
        new ModuleClient({
          clientSecret: CLIENT_SECRET,
          encryptionKey: 'zz',
          baseUrl: BASE_URL,
          fetch: mockFetch(() => new Response()),
        }),
    ).toThrow(/64 hexadecimal/);
    expect(
      () =>
        new ModuleClient({
          clientSecret: '',
          encryptionKey: ENCRYPTION_KEY,
          baseUrl: BASE_URL,
          fetch: mockFetch(() => new Response()),
        }),
    ).toThrow(/clientSecret is required/);
  });
});

describe('ModuleClient refresh', () => {
  it('derives the module path from the client secret and accepts a valid artifact', async () => {
    const bytes = artifact();
    const fetchImpl = mockFetch(() => encryptedResponse(bytes, 'token-1', '"v1"'));
    const client = createClient(fetchImpl);

    expect(client.current).toBeNull();
    const outcome = await client.refresh();

    expect(fetchImpl.requests[0].url).toBe(`${BASE_URL}/modules/${SECRET_SHA256_HEX}.wasm.enc`);
    expect(fetchImpl.requests[0].ifNoneMatch).toBeNull();
    expect(outcome).toMatchObject({ type: 'accepted', change: 'initial' });
    const current = (outcome as { current: { logToken: string } }).current;
    expect(current.logToken).toBe('token-1');
    expect(bytesEqual(client.current!.moduleBytes, bytes)).toBe(true);
    expect(client.current!.etag).toBe('"v1"');
    expect(client.current!.metadata.compilerVersion).toBe('1.2.3');
  });

  it('sends If-None-Match after an accept and treats 304 as unchanged', async () => {
    const bytes = artifact();
    const fetchImpl = mockFetch(call =>
      call === 0 ? encryptedResponse(bytes, 'token-1', '"v1"') : new Response(null, { status: 304 }),
    );
    const client = createClient(fetchImpl);
    await client.refresh();

    const outcome = await client.refresh();
    expect(fetchImpl.requests[1].ifNoneMatch).toBe('"v1"');
    expect(outcome).toEqual({ type: 'unchanged' });
    expect(client.current!.logToken).toBe('token-1');
  });

  it('treats 404 as not-published and keeps serving', async () => {
    const bytes = artifact();
    const fetchImpl = mockFetch(call =>
      call === 0 ? encryptedResponse(bytes, 'token-1', '"v1"') : new Response(null, { status: 404 }),
    );
    const client = createClient(fetchImpl);
    await client.refresh();

    const outcome = await client.refresh();
    expect(outcome).toEqual({ type: 'not-published' });
    expect(client.current).not.toBeNull();
  });

  it('treats 404 before the first accept as not-published with no current', async () => {
    const fetchImpl = mockFetch(() => new Response(null, { status: 404 }));
    const client = createClient(fetchImpl);
    expect(await client.refresh()).toEqual({ type: 'not-published' });
    expect(client.current).toBeNull();
  });

  it('accepts a wrapper-only change: same module bytes identity, new token', async () => {
    const bytes = artifact();
    const fetchImpl = mockFetch(call =>
      encryptedResponse(bytes, call === 0 ? 'token-1' : 'token-2', call === 0 ? '"v1"' : '"v2"'),
    );
    const client = createClient(fetchImpl);
    await client.refresh();
    const firstBytes = client.current!.moduleBytes;

    const outcome = await client.refresh();
    expect(outcome).toMatchObject({ type: 'accepted', change: 'token' });
    expect(client.current!.logToken).toBe('token-2');
    expect(client.current!.moduleBytes).toBe(firstBytes); // identity-stable: nothing to recompile
    expect(client.current!.etag).toBe('"v2"');
  });

  it('accepts a new module body as a full change', async () => {
    const fetchImpl = mockFetch(call =>
      encryptedResponse(
        call === 0 ? artifact() : artifact([CLIENT_SECRET], 2),
        'token-1',
        call === 0 ? '"v1"' : '"v2"',
      ),
    );
    const client = createClient(fetchImpl);
    await client.refresh();

    const outcome = await client.refresh();
    expect(outcome).toMatchObject({ type: 'accepted', change: 'module' });
  });

  it('keeps the last good module on every failure mode', async () => {
    const bytes = artifact();
    const failureResponses = [
      () => new Response(new Uint8Array([1, 2, 3]), { status: 200, headers: { etag: '"bad"' } }), // decrypt garbage
      () => encryptedResponse(new Uint8Array(0), 'token', '"empty"'), // empty module body
      () => encryptedResponse(wasm(), 'token', '"legacy"'), // no metadata section
      () => encryptedResponse(artifact(['someone-elses-secret']), 'token', '"wrong-credential"'),
      () => new Response(null, { status: 500 }),
      () => {
        throw new Error('network down');
      },
    ];
    const fetchImpl = mockFetch(call =>
      call === 0 ? encryptedResponse(bytes, 'token-1', '"v1"') : failureResponses[call - 1](),
    );
    const client = createClient(fetchImpl);
    await client.refresh();
    const accepted = client.current!;

    const reasons: string[] = [];
    for (let i = 0; i < failureResponses.length; i++) {
      const outcome = await client.refresh();
      expect(outcome.type, `failure #${i}`).toBe('failed');
      reasons.push((outcome as { reason: string }).reason);
    }
    expect(reasons[0]).toMatch(/decrypt/);
    expect(reasons[1]).toMatch(/empty module body/);
    expect(reasons[2]).toMatch(/no confidence\.metadata/);
    expect(reasons[3]).toMatch(/does not accept this client secret/);
    expect(reasons[4]).toMatch(/unexpected status 500/);
    expect(reasons[5]).toMatch(/network down/);
    expect(client.current).toBe(accepted); // untouched through every failure
  });

  it('rejects with an aborted signal instead of reporting a failure', async () => {
    const fetchImpl = mockFetch(() => encryptedResponse(artifact(), 'token-1', '"v1"'));
    const client = createClient(fetchImpl);
    const controller = new AbortController();
    controller.abort();
    await expect(client.refresh(controller.signal)).rejects.toThrow(/aborted/);
    expect(client.current).toBeNull();
  });

  it('serializes concurrent refreshes', async () => {
    const fetchImpl = mockFetch(call => encryptedResponse(artifact(), `token-${call + 1}`, `"v${call + 1}"`));
    const client = createClient(fetchImpl);
    const [first, second] = await Promise.all([client.refresh(), client.refresh()]);
    expect(first).toMatchObject({ type: 'accepted', change: 'initial' });
    expect(second).toMatchObject({ type: 'accepted', change: 'token' });
    expect(fetchImpl.requests).toHaveLength(2);
  });

  it('handles a missing etag header', async () => {
    const fetchImpl = mockFetch(() => {
      const response = encryptedResponse(artifact(), 'token-1', '"v1"');
      return new Response(response.body, { status: 200 }); // no etag header
    });
    const client = createClient(fetchImpl);
    await client.refresh();
    expect(client.current!.etag).toBeNull();

    await client.refresh();
    expect(fetchImpl.requests[1].ifNoneMatch).toBeNull();
  });
});
