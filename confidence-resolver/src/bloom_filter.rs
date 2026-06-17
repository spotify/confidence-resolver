use bitvec::prelude as bv;
use fastmurmur3::murmur3_x64_128;

use crate::err::Fallible;
use crate::proto::confidence::flags::admin::v1::resolver_state::BloomFilter as BloomFilterPb;

const STRATEGY_MURMUR128_MITZ_64: i32 = 1;

#[derive(Debug)]
pub struct BloomFilter {
    bits: bv::BitVec<u8, bv::Lsb0>,
    num_hash_functions: u32,
    num_bits: u64,
}

impl BloomFilter {
    pub fn from_proto(packed: BloomFilterPb) -> Fallible<Self> {
        if packed.strategy != STRATEGY_MURMUR128_MITZ_64 {
            return Err(crate::err::ErrorCode::from_tag(
                "bloom_filter.unsupported_strategy",
            ));
        }
        let num_bits = packed.bit_count as u64;
        let num_hash_functions = packed.hash_function_count as u32;
        let mut data = packed.data;
        // Guava serializes its long[] array in big-endian byte order (DataOutputStream.writeLong).
        // BitVec<u8, Lsb0> expects bit 0 at the LSB of byte 0 (little-endian within each long).
        // Reverse each 8-byte chunk to convert from BE to LE so bit indices align.
        for chunk in data.chunks_exact_mut(8) {
            chunk.reverse();
        }
        let bits = bv::BitVec::from_vec(data);
        Ok(BloomFilter {
            bits,
            num_hash_functions,
            num_bits,
        })
    }

    /// Guava MURMUR128_MITZ_64 compatible membership test.
    ///
    /// Matches Guava's BloomFilterStrategies.MURMUR128_MITZ_64 exactly:
    ///   combinedHash = h1; for 0..k { probe(combinedHash); combinedHash += h2; }
    /// Two details that differ from the Kirsch-Mitzenmacher paper:
    ///   - Loop is 0-indexed (starts at h1), not 1-indexed (h1 + h2).
    ///   - Bit index uses `(combined & Long.MAX_VALUE) % numBits` (mask sign bit),
    ///     not `(combined as u64) % numBits` (reinterpret all 64 bits as unsigned).
    #[allow(clippy::arithmetic_side_effects)]
    pub fn might_contain(&self, key: &str) -> bool {
        let hash128 = murmur3_x64_128(key.as_bytes(), 0);
        let h1 = hash128 as i64;
        let h2 = (hash128 >> 64) as i64;
        let mut combined = h1;
        for _ in 0..self.num_hash_functions {
            let bit_index = ((combined & i64::MAX) as u64).wrapping_rem(self.num_bits) as usize;
            match self.bits.get(bit_index) {
                Some(bit) if *bit => {}
                _ => return false,
            }
            combined = combined.wrapping_add(h2);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_packed(data: &[u8], hash_function_count: i32, bit_count: i64) -> BloomFilterPb {
        BloomFilterPb {
            materialized_segment: "segments/test".into(),
            data: data.to_vec(),
            hash_function_count,
            bit_count,
            strategy: STRATEGY_MURMUR128_MITZ_64,
        }
    }

    #[test]
    fn all_bits_set_always_matches() {
        let data = vec![0xFF; 16];
        let packed = make_packed(&data, 3, 128);
        let bf = BloomFilter::from_proto(packed).unwrap();
        assert!(bf.might_contain("anything"));
        assert!(bf.might_contain("something_else"));
    }

    #[test]
    fn all_bits_clear_never_matches() {
        let data = vec![0x00; 16];
        let packed = make_packed(&data, 3, 128);
        let bf = BloomFilter::from_proto(packed).unwrap();
        assert!(!bf.might_contain("anything"));
        assert!(!bf.might_contain("something_else"));
    }

    /// Sets bits using big-endian long layout (matching Guava's serialization).
    fn insert_into_bloom_be(bits: &mut [u8], key: &str, num_hash_functions: u32, num_bits: u64) {
        let hash128 = murmur3_x64_128(key.as_bytes(), 0);
        let h1 = hash128 as i64;
        let h2 = (hash128 >> 64) as i64;
        let mut combined = h1;
        for _ in 0..num_hash_functions {
            let bit_index = ((combined & i64::MAX) as u64).wrapping_rem(num_bits) as usize;
            let long_index = bit_index / 64;
            let bit_within_long = bit_index % 64;
            let byte_within_long = 7 - (bit_within_long / 8);
            let bit_within_byte = bit_within_long % 8;
            let byte_offset = long_index * 8 + byte_within_long;
            bits[byte_offset] |= 1 << bit_within_byte;
            combined = combined.wrapping_add(h2);
        }
    }

    #[test]
    fn programmatic_bloom_filter() {
        let num_bits: u64 = 256;
        let num_hash_functions: u32 = 3;
        let mut data = vec![0u8; (num_bits as usize + 7) / 8];

        insert_into_bloom_be(&mut data, "user1", num_hash_functions, num_bits);
        insert_into_bloom_be(&mut data, "user2", num_hash_functions, num_bits);

        let packed = make_packed(&data, num_hash_functions as i32, num_bits as i64);
        let bf = BloomFilter::from_proto(packed).unwrap();

        assert!(bf.might_contain("user1"));
        assert!(bf.might_contain("user2"));
        assert!(!bf.might_contain("user3"));
        assert!(!bf.might_contain("user999"));
    }

    #[test]
    fn guava_bloom_filter_conformance() {
        use base64::{engine::general_purpose::STANDARD, Engine};

        // Bloom filter generated by backend (Guava 33.5.0), extracted from
        // a real resolver state. Contains keys "user_1" through "user_1000".
        let data = STANDARD
            .decode(include_str!(
                "../test-payloads/bloom_filter_thousand_users.base64"
            ))
            .unwrap();

        let bf_pb = BloomFilterPb {
            materialized_segment: "materializedSegments/thousand-user-segment".into(),
            data,
            hash_function_count: 27,
            bit_count: 38400,
            strategy: STRATEGY_MURMUR128_MITZ_64,
        };
        let bf = BloomFilter::from_proto(bf_pb).unwrap();

        // Keys in the bloom filter: "user_1" through "user_1000"
        assert!(bf.might_contain("user_1"));
        assert!(bf.might_contain("user_459"));
        assert!(bf.might_contain("user_1000"));

        // Keys NOT in the bloom filter
        assert!(!bf.might_contain("user_1001"));
        assert!(!bf.might_contain("user_1003"));
        assert!(!bf.might_contain("not_a_user"));
    }

    #[test]
    fn rejects_unsupported_strategy() {
        let packed = BloomFilterPb {
            materialized_segment: "segments/test".into(),
            data: vec![],
            hash_function_count: 3,
            bit_count: 128,
            strategy: 99,
        };
        assert!(BloomFilter::from_proto(packed).is_err());
    }
}
