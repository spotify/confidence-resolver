use bitvec::prelude as bv;
use fastmurmur3::murmur3_x64_128;

use crate::err::Fallible;
use crate::gzip::decompress_gz;
use crate::proto::confidence::flags::admin::v1::resolver_state::PackedBloomFilter;

const STRATEGY_MURMUR128_MITZ_64: i32 = 1;

#[derive(Debug)]
pub struct BloomFilter {
    bits: bv::BitVec<u8, bv::Lsb0>,
    num_hash_functions: u32,
    num_bits: u64,
}

impl BloomFilter {
    pub fn from_packed(packed: &PackedBloomFilter) -> Fallible<Self> {
        if packed.strategy != STRATEGY_MURMUR128_MITZ_64 {
            return Err(crate::err::ErrorCode::from_tag(
                "bloom_filter.unsupported_strategy",
            ));
        }
        let num_bits = packed.num_bits as u64;
        let num_hash_functions = packed.num_hash_functions as u32;
        let mut decompressed = decompress_gz(&packed.gzipped_data)?;
        // Guava serializes its long[] array in big-endian byte order (DataOutputStream.writeLong).
        // BitVec<u8, Lsb0> expects bit 0 at the LSB of byte 0 (little-endian within each long).
        // Reverse each 8-byte chunk to convert from BE to LE so bit indices align.
        for chunk in decompressed.chunks_exact_mut(8) {
            chunk.reverse();
        }
        let bits = bv::BitVec::from_slice(&decompressed);
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

    fn make_packed(
        data: &[u8],
        num_hash_functions: i32,
        num_bits: i64,
    ) -> PackedBloomFilter {
        use miniz_oxide::deflate::compress_to_vec;

        let compressed = compress_to_vec(data, 6);

        // Build a minimal gzip wrapper: header(10) + compressed + trailer(8)
        let crc = crc32fast::hash(data);
        let isize_val = data.len() as u32;

        let mut gzipped = Vec::new();
        gzipped.extend_from_slice(&[0x1f, 0x8b, 0x08, 0x00, 0, 0, 0, 0, 0, 0xff]);
        gzipped.extend_from_slice(&compressed);
        gzipped.extend_from_slice(&crc.to_le_bytes());
        gzipped.extend_from_slice(&isize_val.to_le_bytes());

        PackedBloomFilter {
            materialized_segment: "segments/test".into(),
            gzipped_data: gzipped,
            num_hash_functions,
            num_bits,
            strategy: STRATEGY_MURMUR128_MITZ_64,
        }
    }

    #[test]
    fn all_bits_set_always_matches() {
        let data = vec![0xFF; 16];
        let packed = make_packed(&data, 3, 128);
        let bf = BloomFilter::from_packed(&packed).unwrap();
        assert!(bf.might_contain("anything"));
        assert!(bf.might_contain("something_else"));
    }

    #[test]
    fn all_bits_clear_never_matches() {
        let data = vec![0x00; 16];
        let packed = make_packed(&data, 3, 128);
        let bf = BloomFilter::from_packed(&packed).unwrap();
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
        let bf = BloomFilter::from_packed(&packed).unwrap();

        assert!(bf.might_contain("user1"));
        assert!(bf.might_contain("user2"));
        assert!(!bf.might_contain("user3"));
        assert!(!bf.might_contain("user999"));
    }

    #[test]
    fn guava_bloom_filter_conformance() {
        use base64::{engine::general_purpose::STANDARD, Engine};

        // Bloom filter generated by Guava 33.5.0:
        //   BloomFilter.create(Funnels.stringFunnel(UTF_8), 100, 0.01)
        //   put("user1"), put("alice"), put("bob")
        //   Serialized via writeTo, header stripped, raw longs gzipped.
        let gzipped_b64 = "H4sIAAAAAAAA/1NgAAIFEMHACCIEGJBBA4MDkFQCMR0UECphgAOkCyqEJC0ApllQFCLzQAAAULYFUXgAAAA=";
        let gzipped_data = STANDARD.decode(gzipped_b64).unwrap();

        let packed = PackedBloomFilter {
            materialized_segment: "materializations/test".into(),
            gzipped_data,
            num_hash_functions: 7,
            num_bits: 960,
            strategy: STRATEGY_MURMUR128_MITZ_64,
        };
        let bf = BloomFilter::from_packed(&packed).unwrap();

        // These were inserted — Guava confirms mightContain == true
        assert!(bf.might_contain("user1"), "user1 should match");
        assert!(bf.might_contain("alice"), "alice should match");
        assert!(bf.might_contain("bob"), "bob should match");

        // These were NOT inserted — Guava confirms mightContain == false
        assert!(!bf.might_contain("carol"), "carol should not match");
        assert!(!bf.might_contain("user3"), "user3 should not match");
    }

    #[test]
    fn rejects_unsupported_strategy() {
        let packed = PackedBloomFilter {
            materialized_segment: "segments/test".into(),
            gzipped_data: vec![],
            num_hash_functions: 3,
            num_bits: 128,
            strategy: 99,
        };
        assert!(BloomFilter::from_packed(&packed).is_err());
    }
}
