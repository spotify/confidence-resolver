import { describe, it, expect } from 'vitest';
import { base64FromBytes, bytesFromBase64 } from './util';

describe('base64FromBytes', () => {
  it('encodes empty array', () => {
    expect(base64FromBytes(new Uint8Array([]))).toBe('');
  });

  it('encodes simple ASCII text', () => {
    const bytes = new TextEncoder().encode('hello');
    expect(base64FromBytes(bytes)).toBe('aGVsbG8=');
  });

  it('encodes binary data', () => {
    const bytes = new Uint8Array([0, 1, 2, 255, 254, 253]);
    expect(base64FromBytes(bytes)).toBe('AAEC//79');
  });

  it('encodes longer text', () => {
    const bytes = new TextEncoder().encode('The quick brown fox jumps over the lazy dog');
    expect(base64FromBytes(bytes)).toBe('VGhlIHF1aWNrIGJyb3duIGZveCBqdW1wcyBvdmVyIHRoZSBsYXp5IGRvZw==');
  });
});

describe('bytesFromBase64', () => {
  it('decodes empty string', () => {
    expect(bytesFromBase64('')).toEqual(new Uint8Array([]));
  });

  it('decodes simple ASCII text', () => {
    const result = bytesFromBase64('aGVsbG8=');
    expect(new TextDecoder().decode(result)).toBe('hello');
  });

  it('decodes binary data', () => {
    const result = bytesFromBase64('AAEC//79');
    expect(result).toEqual(new Uint8Array([0, 1, 2, 255, 254, 253]));
  });

  it('decodes longer text', () => {
    const result = bytesFromBase64('VGhlIHF1aWNrIGJyb3duIGZveCBqdW1wcyBvdmVyIHRoZSBsYXp5IGRvZw==');
    expect(new TextDecoder().decode(result)).toBe('The quick brown fox jumps over the lazy dog');
  });
});

describe('base64 round-trip', () => {
  it('round-trips empty array', () => {
    const original = new Uint8Array([]);
    expect(bytesFromBase64(base64FromBytes(original))).toEqual(original);
  });

  it('round-trips ASCII text', () => {
    const original = new TextEncoder().encode('hello world');
    expect(bytesFromBase64(base64FromBytes(original))).toEqual(original);
  });

  it('round-trips binary data with all byte values', () => {
    const original = new Uint8Array(256);
    for (let i = 0; i < 256; i++) {
      original[i] = i;
    }
    expect(bytesFromBase64(base64FromBytes(original))).toEqual(original);
  });

  it('round-trips random-ish binary data', () => {
    const original = new Uint8Array([72, 101, 108, 108, 111, 0, 255, 128, 64, 32, 16, 8, 4, 2, 1]);
    expect(bytesFromBase64(base64FromBytes(original))).toEqual(original);
  });
});
