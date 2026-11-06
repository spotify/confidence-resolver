import { describe, expect, it } from 'vitest';
import { ABSENT_SLICE, isAbsentSlice, packSlice, unpackSlice } from './slice';

describe('packSlice', () => {
  it('packs pointer into the low 32 bits and length into the high 32 bits', () => {
    expect(packSlice(0x1234, 0x5678)).toBe(0x5678n * 2n ** 32n + 0x1234n);
  });

  it('packs the maximum u32 pointer and length', () => {
    expect(packSlice(0xffffffff, 0xffffffff)).toBe(0xffffffffffffffffn);
  });

  it('produces the absent slice for a zero-length slice at pointer zero', () => {
    expect(packSlice(0, 0)).toBe(ABSENT_SLICE);
    expect(packSlice(0, 0)).toBe(0n);
  });

  it('rejects a non-zero length at pointer zero', () => {
    expect(() => packSlice(0, 5)).toThrow('pointer 0 is the absent slice');
  });

  it('allows a zero-length slice at a non-zero pointer', () => {
    // Do not normalize: the pointer may still need freeing.
    expect(packSlice(7, 0)).toBe(7n);
  });

  it('rejects values outside u32 for pointer and length', () => {
    for (const [ptr, len] of [
      [-1, 0],
      [0, -1],
      [2 ** 32, 0],
      [0, 2 ** 32],
      [1.5, 0],
      [0, 1.5],
      [Number.NaN, 0],
      [Number.POSITIVE_INFINITY, 0],
    ] as const) {
      expect(() => packSlice(ptr, len), `ptr=${ptr} len=${len}`).toThrow(/must be an integer/);
    }
  });
});

describe('unpackSlice', () => {
  it('round-trips representative pointer/length pairs', () => {
    const values = [0, 1, 127, 128, 255, 256, 65535, 65536, 2 ** 24, 2 ** 31, 2 ** 32 - 1];
    for (const ptr of values) {
      for (const len of values) {
        if (ptr === 0 && len > 0) continue; // invalid combination, rejected on pack
        expect(unpackSlice(packSlice(ptr, len))).toEqual({ ptr, len });
      }
    }
  });

  it('unpacks the absent slice to a zero-length slice at pointer zero', () => {
    expect(unpackSlice(ABSENT_SLICE)).toEqual({ ptr: 0, len: 0 });
  });

  it('unpacks arbitrary 64-bit values', () => {
    expect(unpackSlice(0x0000_0001_0000_0002n)).toEqual({ ptr: 2, len: 1 });
    expect(unpackSlice(0xffff_ffff_0000_0001n)).toEqual({ ptr: 1, len: 0xffffffff });
  });

  it('rejects values outside the unsigned 64-bit range', () => {
    expect(() => unpackSlice(-1n)).toThrow(/unsigned 64-bit/);
    expect(() => unpackSlice(2n ** 64n)).toThrow(/unsigned 64-bit/);
  });
});

describe('isAbsentSlice', () => {
  it('is true only for the absent slice', () => {
    expect(isAbsentSlice(ABSENT_SLICE)).toBe(true);
    expect(isAbsentSlice(0n)).toBe(true);
    expect(isAbsentSlice(packSlice(1, 0))).toBe(false);
    expect(isAbsentSlice(packSlice(0, 0) | 1n)).toBe(false);
  });
});
