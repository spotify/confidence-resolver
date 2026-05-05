import { createCipheriv, createDecipheriv, createHash, randomBytes } from 'node:crypto';

// AES-256-GCM. The resolve token carries the full evaluation context and the
// resolved variants — never let the client see it. We seal it with a server
// key so the client can only round-trip an opaque handle through the apply API
// route. Mirrors what App Router gets for free via encrypted server actions.

const ALGO = 'aes-256-gcm';
const IV_LEN = 12;
const TAG_LEN = 16;

let cachedKey: Buffer | null = null;
function getKey(): Buffer {
  if (cachedKey) return cachedKey;
  const raw = process.env.CONFIDENCE_TOKEN_KEY;
  if (!raw) {
    throw new Error(
      'CONFIDENCE_TOKEN_KEY is not set. Generate one with: openssl rand -hex 32 — and set it in the server environment.',
    );
  }
  cachedKey = createHash('sha256').update(raw).digest();
  return cachedKey;
}

export function sealResolveToken(token: string): string {
  const iv = randomBytes(IV_LEN);
  const cipher = createCipheriv(ALGO, getKey(), iv);
  const ciphertext = Buffer.concat([cipher.update(token, 'utf8'), cipher.final()]);
  const tag = cipher.getAuthTag();
  return Buffer.concat([iv, tag, ciphertext]).toString('base64url');
}

export function openResolveToken(handle: string): string {
  const buf = Buffer.from(handle, 'base64url');
  if (buf.length < IV_LEN + TAG_LEN) {
    throw new Error('Invalid Confidence handle');
  }
  const iv = buf.subarray(0, IV_LEN);
  const tag = buf.subarray(IV_LEN, IV_LEN + TAG_LEN);
  const ciphertext = buf.subarray(IV_LEN + TAG_LEN);
  const decipher = createDecipheriv(ALGO, getKey(), iv);
  decipher.setAuthTag(tag);
  return Buffer.concat([decipher.update(ciphertext), decipher.final()]).toString('utf8');
}

/** @internal Test-only: clear the cached key so a fresh env var is picked up. */
export function __resetKeyCacheForTests(): void {
  cachedKey = null;
}
