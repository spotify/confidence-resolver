/**
 * Shared crypto routines for artifact decryption and key validation.
 *
 * Both the state-based provider (encrypted resolver state) and the
 * compiled-module provider (encrypted ClientModule wrapper) use the same
 * AES-256-GCM wire format:
 *
 * ```
 * 12-byte nonce || ciphertext || 16-byte tag      (no key prefix, empty AAD)
 * ```
 */

/** Decrypt an AES-256-GCM payload: 12-byte nonce || ciphertext || 16-byte tag. */
export async function decryptAesGcm(data: Uint8Array, rawKey: Uint8Array): Promise<Uint8Array> {
  const NONCE_LEN = 12;
  if (data.length < NONCE_LEN) {
    throw new Error('Encrypted payload too short (missing nonce)');
  }
  const iv = data.buffer.slice(data.byteOffset, data.byteOffset + NONCE_LEN) as ArrayBuffer;
  const ciphertext = data.buffer.slice(data.byteOffset + NONCE_LEN, data.byteOffset + data.byteLength) as ArrayBuffer;
  const key = await crypto.subtle.importKey('raw', rawKey.buffer as ArrayBuffer, 'AES-GCM', false, ['decrypt']);
  const plaintext = await crypto.subtle.decrypt({ name: 'AES-GCM', iv }, key, ciphertext);
  return new Uint8Array(plaintext);
}

/** Validate before allocating WASM resources or starting network activity. */
export function validateEncryptionKey(key: string): void {
  if (typeof key !== 'string' || key.length !== 64 || !/^[0-9a-fA-F]+$/.test(key)) {
    throw new Error('encryptionKey is required and must contain exactly 64 hexadecimal characters');
  }
}
