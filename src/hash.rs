use core::hash::Hasher;
use frozen_core::hints;
use twox_hash::XxHash32;

pub(crate) use crate::common::{MIN_VALID_HASH, SEED};

pub(crate) const KV_HASH_MASK: u32 = 0x7FFF_FFFF;
#[allow(dead_code)]
pub(crate) const LIST_HASH_BIT: u32 = 0x8000_0000;

/// Hash a key for Key-Value storage (bit 31 cleared to 0)
///
/// Ref: `DESIGN.md` §4.1
#[inline(always)]
pub(crate) fn hash_kv_key(key: &[u8]) -> u32 {
    let mut hasher = XxHash32::with_seed(SEED);

    hasher.write(key);
    let raw = hasher.finish() as u32;
    let h = raw & KV_HASH_MASK;

    if hints::unlikely(h < MIN_VALID_HASH) { h + MIN_VALID_HASH } else { h }
}

/// Hash a key for List storage (bit 31 set to 1)
///
/// Ref: `DESIGN.md` §4.1
#[allow(dead_code)]
#[inline(always)]
pub(crate) fn hash_list_key(key: &[u8]) -> u32 {
    let mut hasher = XxHash32::with_seed(SEED);

    hasher.write(key);
    let raw = hasher.finish() as u32;

    raw | LIST_HASH_BIT
}

/// Default key hasher routing to Key-Value keyspace
#[inline(always)]
pub(crate) fn hash_key(key: &[u8]) -> u32 {
    hash_kv_key(key)
}

/// Trie child index calculation via folded 4-bit slicing
///
/// Ref: `DESIGN.md` §4.2
#[inline(always)]
pub(crate) const fn trie_child_idx(hash: u32, depth: usize) -> usize {
    let raw_slice = (hash >> (4 * (depth % 8))) & 0x0F;
    if raw_slice >= 0x0F { 0 } else { raw_slice as usize }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_constants_and_sentinels() {
        assert_eq!(SEED, 0x00);
        assert_eq!(MIN_VALID_HASH, 2);
        assert_eq!(KV_HASH_MASK, 0x7FFF_FFFF);
        assert_eq!(LIST_HASH_BIT, 0x8000_0000);
    }

    #[test]
    fn ok_deterministic_output() {
        let key = b"persistent_disk_key";
        let expected_kv = hash_kv_key(key);
        let expected_list = hash_list_key(key);

        for _ in 0..0x64 {
            assert_eq!(hash_kv_key(key), expected_kv);
            assert_eq!(hash_key(key), expected_kv);
            assert_eq!(hash_list_key(key), expected_list);
        }
    }

    #[test]
    fn ok_bit31_partitioning_and_disjoint_keyspace() {
        let sample_keys: &[&[u8]] = &[
            b"",
            b"a",
            b"turbofox:kv:test",
            b"user_session_492040182",
            &[0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01],
        ];

        for &key in sample_keys {
            let h_kv = hash_kv_key(key);
            let h_list = hash_list_key(key);

            assert_eq!(h_kv & LIST_HASH_BIT, 0x00);
            assert_eq!(h_list & LIST_HASH_BIT, LIST_HASH_BIT);
            assert_ne!(h_kv, h_list);
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
            let h_kv = hash_kv_key(&key);
            let h_list = hash_list_key(&key);

            assert!(
                h_kv >= MIN_VALID_HASH,
                "KV Hash must be >= MIN_VALID_HASH (2), got {h_kv} for key {key:?}"
            );
            assert!(
                h_list >= LIST_HASH_BIT,
                "List Hash must have bit 31 set, got {h_list} for key {key:?}"
            );
        }
    }

    #[test]
    fn ok_normalization_mapping() {
        fn normalize_kv(raw: u32) -> u32 {
            let h = raw & KV_HASH_MASK;
            if h < MIN_VALID_HASH { h + MIN_VALID_HASH } else { h }
        }

        fn normalize_list(raw: u32) -> u32 {
            raw | LIST_HASH_BIT
        }

        assert_eq!(normalize_kv(0), 2);
        assert_eq!(normalize_kv(1), 3);
        assert_eq!(normalize_kv(2), 2);
        assert_eq!(normalize_kv(3), 3);
        assert_eq!(normalize_kv(0x64), 0x64);
        assert_eq!(normalize_kv(0x7FFF_FFFF), 0x7FFF_FFFF);
        assert_eq!(normalize_kv(0x8000_0000), 2);
        assert_eq!(normalize_kv(0x8000_0001), 3);
        assert_eq!(normalize_kv(u32::MAX), 0x7FFF_FFFF);

        assert_eq!(normalize_list(0), 0x8000_0000);
        assert_eq!(normalize_list(1), 0x8000_0001);
        assert_eq!(normalize_list(0x7FFF_FFFF), u32::MAX);
        assert_eq!(normalize_list(0x8000_0000), 0x8000_0000);
        assert_eq!(normalize_list(u32::MAX), u32::MAX);
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

            let h = raw & KV_HASH_MASK;
            let expected_kv = if h < MIN_VALID_HASH { h + MIN_VALID_HASH } else { h };
            assert_eq!(hash_kv_key(key), expected_kv);
            assert_eq!(hash_key(key), expected_kv);

            let expected_list = raw | LIST_HASH_BIT;
            assert_eq!(hash_list_key(key), expected_list);
        }
    }

    #[test]
    fn err_different_keys_produce_different_hashes() {
        let k1 = hash_kv_key(b"key_a");
        let k2 = hash_kv_key(b"key_b");
        let k3 = hash_kv_key(b"key_c");

        assert_ne!(k1, k2);
        assert_ne!(k1, k3);
        assert_ne!(k2, k3);

        let l1 = hash_list_key(b"key_a");
        let l2 = hash_list_key(b"key_b");
        let l3 = hash_list_key(b"key_c");

        assert_ne!(l1, l2);
        assert_ne!(l1, l3);
        assert_ne!(l2, l3);
    }

    #[test]
    fn err_sensitivity_to_single_bit_flip() {
        let base = b"sensitive_key_test_0";
        let mut modified = *base;
        modified[base.len() - 1] ^= 1;

        assert_ne!(hash_kv_key(base), hash_kv_key(&modified));
        assert_ne!(hash_list_key(base), hash_list_key(&modified));
    }

    #[test]
    fn err_prefix_and_suffix_sensitivity() {
        assert_ne!(hash_kv_key(b"prefix"), hash_kv_key(b"prefix_extended"));
        assert_ne!(hash_kv_key(b"\0value"), hash_kv_key(b"value\0"));
        assert_ne!(hash_kv_key(b"a"), hash_kv_key(b"aa"));

        assert_ne!(hash_list_key(b"prefix"), hash_list_key(b"prefix_extended"));
        assert_ne!(hash_list_key(b"\0value"), hash_list_key(b"value\0"));
        assert_ne!(hash_list_key(b"a"), hash_list_key(b"aa"));
    }

    #[test]
    fn ok_empty_key() {
        let h_kv = hash_kv_key(b"");
        assert!(h_kv >= MIN_VALID_HASH);
        assert_eq!(h_kv & LIST_HASH_BIT, 0x00);

        let h_list = hash_list_key(b"");
        assert!(h_list >= LIST_HASH_BIT);
        assert_eq!(h_list & LIST_HASH_BIT, LIST_HASH_BIT);
    }

    #[test]
    fn ok_various_key_lengths() {
        for len in
            [1, 2, 4, 8, 0x0F, 0x10, 0x11, 0x1F, 0x20, 0x21, 0x40, 0x80, 0x0100, 0x0400, 0x1000]
        {
            let key = vec![0x3C; len];
            let h_kv = hash_kv_key(&key);
            assert!(h_kv >= MIN_VALID_HASH);
            assert_eq!(h_kv & LIST_HASH_BIT, 0x00);

            let h_list = hash_list_key(&key);
            assert!(h_list >= LIST_HASH_BIT);
            assert_eq!(h_list & LIST_HASH_BIT, LIST_HASH_BIT);
        }
    }

    #[test]
    fn ok_trie_depth_slicing_and_folding_invariant() {
        let h_kv = hash_kv_key(b"trie_routing_key");
        let h_list = hash_list_key(b"trie_routing_key");

        for depth in 0..0x10 {
            let child_kv = trie_child_idx(h_kv, depth);
            assert!(child_kv < 15, "Trie child index must be in 0..14");

            let child_list = trie_child_idx(h_list, depth);
            assert!(child_list < 15, "Trie child index must be in 0..14");
        }

        // Test folding edge cases
        let mock_h15 = 15u32;
        assert_eq!(trie_child_idx(mock_h15, 0), 0);
        assert_eq!(trie_child_idx(14, 0), 14);

        // Depth 7 partitioning verification
        let slice_kv = (h_kv >> (4 * (7 % 8))) & 0x0F;
        assert!(slice_kv <= 7, "KV slice at depth 7 must have bit 3 = 0");

        let slice_list = (h_list >> (4 * (7 % 8))) & 0x0F;
        assert!(slice_list >= 8, "List slice at depth 7 must have bit 3 = 1");
    }
}
