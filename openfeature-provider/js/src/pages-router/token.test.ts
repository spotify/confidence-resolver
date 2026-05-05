import { afterEach, beforeAll, describe, expect, it } from 'vitest';
import { __resetKeyCacheForTests, openResolveToken, sealResolveToken } from './token';

describe('sealResolveToken / openResolveToken', () => {
  beforeAll(() => {
    process.env.CONFIDENCE_TOKEN_KEY = 'test-key-do-not-use-in-prod';
    __resetKeyCacheForTests();
  });

  afterEach(() => {
    process.env.CONFIDENCE_TOKEN_KEY = 'test-key-do-not-use-in-prod';
    __resetKeyCacheForTests();
  });

  it('round-trips a typical resolve token', () => {
    const token = 'abc.def.ghi-some-base64-ish-resolve-token==';
    const sealed = sealResolveToken(token);
    expect(sealed).not.toContain(token);
    expect(openResolveToken(sealed)).toBe(token);
  });

  it('round-trips an empty string', () => {
    const sealed = sealResolveToken('');
    expect(openResolveToken(sealed)).toBe('');
  });

  it('produces a different ciphertext each call (random IV)', () => {
    const token = 'same-input';
    expect(sealResolveToken(token)).not.toBe(sealResolveToken(token));
  });

  it('rejects a tampered ciphertext (auth tag mismatch)', () => {
    const sealed = sealResolveToken('something');
    // Flip a byte in the ciphertext region.
    const buf = Buffer.from(sealed, 'base64url');
    buf[buf.length - 1] ^= 0x01;
    const tampered = buf.toString('base64url');
    expect(() => openResolveToken(tampered)).toThrow();
  });

  it('rejects a too-short handle', () => {
    expect(() => openResolveToken('AAAA')).toThrow('Invalid Confidence handle');
  });

  it('rejects a handle sealed with a different key', () => {
    const sealed = sealResolveToken('payload');
    process.env.CONFIDENCE_TOKEN_KEY = 'a-different-key';
    __resetKeyCacheForTests();
    expect(() => openResolveToken(sealed)).toThrow();
  });

  it('throws if CONFIDENCE_TOKEN_KEY is unset', () => {
    delete process.env.CONFIDENCE_TOKEN_KEY;
    __resetKeyCacheForTests();
    expect(() => sealResolveToken('x')).toThrow(/CONFIDENCE_TOKEN_KEY/);
  });
});
