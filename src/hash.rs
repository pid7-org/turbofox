use core::hash::Hasher;
use frozen_core::hints;
use twox_hash::XxHash32;

pub(crate) use crate::common::{MIN_VALID_HASH, SEED};

#[inline(always)]
pub(crate) fn hash_key(key: &[u8]) -> u32 {
    let mut hasher = XxHash32::with_seed(SEED);

    hasher.write(key);
    let h = hasher.finish() as u32;

    if hints::unlikely(h < MIN_VALID_HASH) { h + MIN_VALID_HASH } else { h }
}

/// Trie child index calculation via folded 4-bit slicing
///
/// Ref: `DESIGN.md` §4.2
#[inline(always)]
pub(crate) const fn trie_child_idx(hash: u32, depth: usize) -> usize {
    let raw_slice = (hash >> (4 * (depth % 8))) & 0x0F;
    if raw_slice >= 15 { 0 } else { raw_slice as usize }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_constants_and_sentinels() {
        assert_eq!(SEED, 0x00);
        assert_eq!(MIN_VALID_HASH, 2);
    }

    #[test]
    fn ok_deterministic_output() {
        let key = b"persistent_disk_key";
        let expected = hash_key(key);
        for _ in 0..0x64 {
            assert_eq!(hash_key(key), expected);
        }
    }

    #[test]
    fn err_sentinel_avoidance() {
        let test_cases: Vec<Vec<u8>> = vec![
            vec![],
            vec![0],
            vec![1],
            vec![0; 4],
            vec![0; 0x10],
            vec![0xFF; 0x10],
            vec![0xAA; 0x20],
            b"test_key".to_vec(),
            (0..=0xFF).collect(),
        ];

        for key in test_cases {
            let h = hash_key(&key);
            assert!(
                h >= MIN_VALID_HASH,
                "Hash must be >= MIN_VALID_HASH (2), got {h} for key {key:?}"
            );
        }
    }

    #[test]
    fn ok_normalization_mapping() {
        fn normalize(raw: u32) -> u32 {
            if raw < MIN_VALID_HASH { raw + MIN_VALID_HASH } else { raw }
        }

        assert_eq!(normalize(0), 2);
        assert_eq!(normalize(1), 3);
        assert_eq!(normalize(2), 2);
        assert_eq!(normalize(3), 3);
        assert_eq!(normalize(0x64), 0x64);
        assert_eq!(normalize(u32::MAX), u32::MAX);
    }

    #[test]
    fn ok_parity_with_xxhash32() {
        let sample_keys: &[&[u8]] = &[
            b"",
            b"a",
            b"turbofox:kv:test",
            b"user_session_492040182",
            &[0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01],
        ];

        for &key in sample_keys {
            let mut hasher = XxHash32::with_seed(SEED);
            hasher.write(key);
            let raw = hasher.finish() as u32;
            let expected = if raw < MIN_VALID_HASH { raw + MIN_VALID_HASH } else { raw };
            assert_eq!(hash_key(key), expected);
        }
    }

    #[test]
    fn err_different_keys_produce_different_hashes() {
        let k1 = hash_key(b"key_a");
        let k2 = hash_key(b"key_b");
        let k3 = hash_key(b"key_c");

        assert_ne!(k1, k2);
        assert_ne!(k1, k3);
        assert_ne!(k2, k3);
    }

    #[test]
    fn err_sensitivity_to_single_bit_flip() {
        let base = b"sensitive_key_test_0";
        let mut modified = *base;
        modified[base.len() - 1] ^= 1;

        assert_ne!(hash_key(base), hash_key(&modified));
    }

    #[test]
    fn err_prefix_and_suffix_sensitivity() {
        assert_ne!(hash_key(b"prefix"), hash_key(b"prefix_extended"));
        assert_ne!(hash_key(b"\0value"), hash_key(b"value\0"));
        assert_ne!(hash_key(b"a"), hash_key(b"aa"));
    }

    #[test]
    fn ok_empty_key() {
        let h = hash_key(b"");
        assert!(h >= MIN_VALID_HASH);
    }

    #[test]
    fn ok_various_key_lengths() {
        for len in
            [1, 2, 4, 8, 0x0F, 0x10, 0x11, 0x1F, 0x20, 0x21, 0x40, 0x80, 0x0100, 0x0400, 0x1000]
        {
            let key = vec![0x3C; len];
            let h = hash_key(&key);
            assert!(h >= MIN_VALID_HASH);
        }
    }

    #[test]
    fn ok_trie_depth_slicing_and_folding_invariant() {
        let h = hash_key(b"trie_routing_key");
        for depth in 0..0x10 {
            let child_idx = trie_child_idx(h, depth);
            assert!(child_idx < 15, "Trie child index must be in 0..14");
        }

        // Test folding edge cases
        let mock_h15 = 15u32;
        let mock_h14 = 14u32;

        assert_eq!(trie_child_idx(mock_h15, 0), 0);
        assert_eq!(trie_child_idx(mock_h14, 0), 14);
    }
}
