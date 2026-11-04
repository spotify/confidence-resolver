/**
 * Packed `Slice` values for the compiled client-module ABI.
 *
 * A `Slice` is a packed 64-bit value referencing a region of the module's
 * linear memory:
 *
 * ```
 * Slice = ptr | (len << 32)     // low 32 bits: pointer, high 32 bits: length
 * ```
 *
 * The value `0n` (a zero-length slice at pointer 0) is the ABI's "absent"
 * value. Ownership follows the provider contract: request slices are
 * allocated by the host and taken over by the module; response slices are
 * allocated by the module and must be freed by the host once read; log
 * payloads are acknowledged through `ack_logs`, never freed.
 */
export type PackedSlice = bigint;

/** The ABI's "absent" slice: zero length at pointer zero. */
export const ABSENT_SLICE: PackedSlice = 0n;

const U32_MASK = 0xffffffffn;
const U32_MAX = 0xffffffff;
const PACKED_MAX = 0xffffffffffffffffn;

function assertU32(what: string, value: number): void {
  if (typeof value !== 'number' || !Number.isInteger(value) || value < 0 || value > U32_MAX) {
    throw new Error(`${what} must be an integer in [0, ${U32_MAX}], got: ${value}`);
  }
}

/** Pack a pointer/length pair into a `Slice`. */
export function packSlice(ptr: number, len: number): PackedSlice {
  assertU32('ptr', ptr);
  assertU32('len', len);
  if (ptr === 0 && len > 0) {
    throw new Error('Cannot pack a slice with ptr=0 and len>0: pointer 0 is the absent slice');
  }
  return BigInt(ptr) | (BigInt(len) << 32n);
}

/** Unpack a `Slice` into its pointer and length. */
export function unpackSlice(packed: PackedSlice): { ptr: number; len: number } {
  if (packed < 0n || packed > PACKED_MAX) {
    throw new Error(`Slice must be an unsigned 64-bit value, got: ${packed}`);
  }
  return {
    ptr: Number(packed & U32_MASK),
    len: Number((packed >> 32n) & U32_MASK),
  };
}

/** True when the slice is the ABI's "absent" value (`0n`). */
export function isAbsentSlice(packed: PackedSlice): boolean {
  return packed === ABSENT_SLICE;
}
