//! Per-user resolver state slicing.
//!
//! Given a full account [`ResolverState`](crate::proto::confidence::flags::admin::v1::ResolverState)
//! and a fixed `(account_id, unit)`, this module produces a slimmed
//! `ResolverState` where everything that depends on the unit at resolve time
//! has been precomputed:
//!
//! - Each segment's population bitset is collapsed to either a
//!   `full_bitset(true)` sentinel (the pinned unit is in the segment
//!   population) or a single gzipped all-zero bitset (the pinned unit is not).
//! - Each rule's variant assignment ranges are rewritten so the assignment the
//!   pinned unit hashes into covers the entire `[0, bucket_count)` range and
//!   the others are empty.
//!
//! The slimmed state can be loaded by the *unmodified* resolver and will
//! produce identical answers to the full state for any evaluation context that
//! uses the same `unit`. The evaluation context may otherwise vary freely
//! (country, app version, etc.).
//!
//! Limitations:
//! - All rules in the state must use the same `targeting_key_selector` (the
//!   empty string and `"targeting_key"` are treated as equivalent). If the
//!   state mixes selectors, the slicer aborts: precomputing membership for
//!   one unit cannot answer the bitset check for a different unit that some
//!   other rule would extract from the evaluation context.
//! - Materializations are out of scope. Rules that read or write
//!   materializations still require the host to supply records at resolve
//!   time; the slicer leaves their definitions unchanged.

use bitvec::prelude as bv;

use crate::err::{Fallible, OrFailExt};
use crate::gzip::decompress_gz;
use crate::has_only_one_full_variant;
use crate::module_err;
use crate::proto::confidence::flags::admin::v1 as flags_admin;
use crate::proto::confidence::flags::admin::v1::flag::rule::{AssignmentSpec, BucketRange};
use crate::proto::confidence::flags::admin::v1::resolver_state::packed_bitset::Bitset;
use crate::proto::confidence::flags::admin::v1::resolver_state::PackedBitset;
use crate::proto::confidence::flags::admin::v1::ResolverState as ResolverStatePb;
use crate::{bucket, hash};

const BUCKETS: u64 = 1_000_000;
const BITSET_BYTES: usize = 125_000;
const DEFAULT_SELECTOR: &str = "targeting_key";

/// Slices a full `ResolverState` into a slimmed state for a single fixed `unit`.
///
/// `unit` is interpreted as the value the resolver would extract from the
/// evaluation context using each rule's `targeting_key_selector`. All rules
/// in the state must use the same effective selector — see module-level docs.
pub fn slice_for_unit(
    mut state: ResolverStatePb,
    account_id: &str,
    unit: &str,
) -> Fallible<ResolverStatePb> {
    require_consistent_selector(&state.flags)?;
    let user_population_bucket = population_bucket(account_id, unit)?;
    state.bitsets = slice_population(state.bitsets, user_population_bucket)?;
    slice_variants(&mut state.flags, unit)?;
    Ok(state)
}

fn require_consistent_selector(flags: &[flags_admin::Flag]) -> Fallible<()> {
    let mut seen: Option<&str> = None;
    for flag in flags {
        for rule in &flag.rules {
            let effective = effective_selector(&rule.targeting_key_selector);
            match seen {
                None => seen = Some(effective),
                Some(s) if s == effective => {}
                Some(_) => return Err(module_err!(":slicer.mixed_selectors")),
            }
        }
    }
    Ok(())
}

fn effective_selector(raw: &str) -> &str {
    if raw.is_empty() {
        DEFAULT_SELECTOR
    } else {
        raw
    }
}

fn population_bucket(account_id: &str, unit: &str) -> Fallible<usize> {
    let salted = format!("MegaSalt-{}|{}", account_id, unit);
    bucket(hash(&salted), BUCKETS)
}

fn slice_population(bitsets: Vec<PackedBitset>, user_bucket: usize) -> Fallible<Vec<PackedBitset>> {
    let mut out = Vec::with_capacity(bitsets.len());
    for packed in bitsets {
        let Some(bs) = packed.bitset else {
            out.push(PackedBitset {
                segment: packed.segment,
                bitset: None,
            });
            continue;
        };
        let new_bitset = match bs {
            Bitset::FullBitset(_) => bs,
            Bitset::GzippedBitset(zipped) => {
                let raw = decompress_gz(&zipped[..])?;
                let bv = bv::BitVec::<u8, bv::Lsb0>::from_slice(&raw);
                let in_segment = bv.get(user_bucket).map(|b| *b).unwrap_or(false);
                if in_segment {
                    Bitset::FullBitset(true)
                } else {
                    let zeros = vec![0u8; BITSET_BYTES];
                    Bitset::GzippedBitset(compress_gz(&zeros))
                }
            }
        };
        out.push(PackedBitset {
            segment: packed.segment,
            bitset: Some(new_bitset),
        });
    }
    Ok(out)
}

fn slice_variants(flags: &mut [flags_admin::Flag], unit: &str) -> Fallible<()> {
    for flag in flags.iter_mut() {
        for rule in flag.rules.iter_mut() {
            let Some(spec) = rule.assignment_spec.as_mut() else {
                continue;
            };
            if has_only_one_full_variant(spec) {
                continue;
            }
            slice_assignment_spec(spec, &rule.segment, unit)?;
        }
    }
    Ok(())
}

fn slice_assignment_spec(
    spec: &mut AssignmentSpec,
    segment_name: &str,
    unit: &str,
) -> Fallible<()> {
    let segment_id = segment_name.split('/').nth(1).or_fail()?;
    let key = format!("{}|{}", segment_id, unit);
    let b = bucket(hash(&key), spec.bucket_count as u64)? as i32;

    let matched_idx = spec.assignments.iter().position(|a| {
        a.bucket_ranges
            .iter()
            .any(|range| range.lower <= b && b < range.upper)
    });

    let Some(matched_idx) = matched_idx else {
        for a in spec.assignments.iter_mut() {
            a.bucket_ranges.clear();
        }
        return Ok(());
    };

    let full_range = BucketRange {
        lower: 0,
        upper: spec.bucket_count,
    };
    for (idx, a) in spec.assignments.iter_mut().enumerate() {
        if idx == matched_idx {
            a.bucket_ranges = vec![full_range.clone()];
        } else {
            a.bucket_ranges.clear();
        }
    }
    Ok(())
}

/// Wraps raw bytes in a minimal gzip envelope that [`decompress_gz`] accepts.
///
/// Header is fixed (no FNAME/FCOMMENT/FEXTRA/FHCRC flags); trailer is the
/// little-endian CRC32 of the uncompressed data followed by its length mod
/// 2^32. This mirrors the parser in `crate::gzip`.
fn compress_gz(data: &[u8]) -> Vec<u8> {
    use miniz_oxide::deflate::compress_to_vec;

    let mut out = Vec::new();
    out.extend_from_slice(&[0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255]);
    out.extend(compress_to_vec(data, 6));
    let crc = crc32fast::hash(data);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gz_roundtrip() {
        let data = vec![0u8; 125_000];
        let compressed = compress_gz(&data);
        let decompressed = decompress_gz(&compressed).unwrap();
        assert_eq!(decompressed, data);
        assert!(compressed.len() < 200, "all-zero gzip should be tiny");
    }

    #[test]
    fn gz_roundtrip_varied() {
        let mut data = vec![0u8; 4096];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i * 31 % 251) as u8;
        }
        let compressed = compress_gz(&data);
        let decompressed = decompress_gz(&compressed).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn effective_selector_treats_empty_as_default() {
        assert_eq!(effective_selector(""), DEFAULT_SELECTOR);
        assert_eq!(effective_selector("targeting_key"), "targeting_key");
        assert_eq!(effective_selector("visitor_id"), "visitor_id");
    }
}
