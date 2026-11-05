import { bytesToHex } from './util';

/** SHA-256 of a string's UTF-8 bytes, as a fresh digest. */
export async function sha256Bytes(input: string): Promise<Uint8Array> {
  const data = new TextEncoder().encode(input);
  const hashBuffer = await crypto.subtle.digest('SHA-256', data);
  return new Uint8Array(hashBuffer);
}

/** SHA-256 of a string's UTF-8 bytes, as lowercase hex. */
export async function sha256Hex(input: string): Promise<string> {
  return bytesToHex(await sha256Bytes(input));
}
