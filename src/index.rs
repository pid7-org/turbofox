use crate::index_probe::{FP_EMPTY, FP_TOMBSTONE, MIN_VALID_FP, index_probe_kernel};
use std::sync::RwLock;

pub(crate) const PROBE_LIMIT: usize = 0x10;
pub(crate) const MIN_CAPACITY: usize = 0x40;
pub(crate) const SHARD_COUNT: usize = 0x20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(C, align(8))]
pub(crate) struct IndexRef {
    pub(crate) hash: u32,
    pub(crate) bucket_slot: u32,
    pub(crate) dbuf_slot: u32,
    pub(crate) entry_idx: u8,
    pub(crate) _pad: [u8; 3],
    pub(crate) offset: u64,
}

impl IndexRef {
    #[inline(always)]
    pub(crate) fn is_list(&self) -> bool {
        (self.offset.to_ne_bytes()[0] & crate::common::LIST_FLAG_MASK) != 0
    }

    #[inline(always)]
    pub(crate) fn klen(&self) -> usize {
        self.offset.to_ne_bytes()[1] as usize
    }

    #[inline(always)]
    pub(crate) fn vlen(&self) -> usize {
        let b = self.offset.to_ne_bytes();
        u16::from_le_bytes([b[2], b[3]]) as usize
    }

    #[inline(always)]
    pub(crate) fn buf_idx(&self) -> u32 {
        let b = self.offset.to_ne_bytes();
        u32::from_le_bytes([b[4], b[5], b[6], b[7]])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LookupResult {
    Hit(IndexRef),
    DefiniteMiss,
    Overflow,
}

#[inline(always)]
pub(crate) const fn compute_fingerprint(hash: u32) -> u16 {
    let fp = (hash >> 0x10) as u16;
    if fp < MIN_VALID_FP { fp.wrapping_add(MIN_VALID_FP) } else { fp }
}

#[derive(Debug)]
pub(crate) struct FpIndexShard {
    fingerprints: Vec<u16>,
    refs: Vec<IndexRef>,
    capacity: usize,
    count: usize,
    tombstones: usize,
}

impl FpIndexShard {
    pub(crate) fn new(desired_capacity: usize) -> Self {
        let capacity = desired_capacity.max(MIN_CAPACITY).next_power_of_two();
        Self {
            fingerprints: vec![FP_EMPTY; capacity],
            refs: vec![IndexRef::default(); capacity],
            capacity,
            count: 0x00,
            tombstones: 0x00,
        }
    }

    #[inline(always)]
    fn primary_slot(&self, hash: u32) -> usize {
        (hash as usize) & (self.capacity - 0x01)
    }

    #[inline(always)]
    fn load_16_fps(&self, primary: usize) -> [u16; PROBE_LIMIT] {
        let mut buf = [0u16; PROBE_LIMIT];

        if primary + PROBE_LIMIT <= self.capacity {
            buf.copy_from_slice(&self.fingerprints[primary..primary + PROBE_LIMIT]);
        } else {
            let head = self.capacity - primary;

            buf[..head].copy_from_slice(&self.fingerprints[primary..self.capacity]);
            buf[head..].copy_from_slice(&self.fingerprints[..PROBE_LIMIT - head]);
        }

        buf
    }

    pub(crate) fn lookup<F>(&self, hash: u32, mut verify: F) -> LookupResult
    where
        F: FnMut(IndexRef) -> bool,
    {
        let target_fp = compute_fingerprint(hash);
        let primary = self.primary_slot(hash);
        let candidate_fps = self.load_16_fps(primary);
        let kernel = index_probe_kernel();
        let mask = unsafe { kernel(&candidate_fps, target_fp) };

        let first_empty_offset = if mask.empty_mask != 0x00 {
            mask.empty_mask.trailing_zeros() as usize
        } else {
            PROBE_LIMIT
        };

        let valid_match_mask = if first_empty_offset < PROBE_LIMIT {
            mask.match_mask & ((1 << first_empty_offset) - 1)
        } else {
            mask.match_mask
        };

        let mut matches = valid_match_mask;
        while matches != 0x00 {
            let off = matches.trailing_zeros() as usize;
            let slot = (primary + off) & (self.capacity - 0x01);
            let r = self.refs[slot];

            if r.hash == hash && verify(r) {
                return LookupResult::Hit(r);
            }

            matches &= matches - 1;
        }

        if first_empty_offset < PROBE_LIMIT {
            LookupResult::DefiniteMiss
        } else {
            LookupResult::Overflow
        }
    }

    pub(crate) fn insert(&mut self, hash: u32, r: IndexRef) {
        if (self.count + self.tombstones + 0x01) * 0x0A >= self.capacity * 0x07 {
            let new_cap = self.capacity * 0x02;
            self.resize(new_cap);
        }

        let target_fp = compute_fingerprint(hash);
        let primary = self.primary_slot(hash);
        let candidate_fps = self.load_16_fps(primary);
        let kernel = index_probe_kernel();
        let mask = unsafe { kernel(&candidate_fps, target_fp) };

        let mut matches = mask.match_mask;
        while matches != 0x00 {
            let off = matches.trailing_zeros() as usize;
            let slot = (primary + off) & (self.capacity - 0x01);

            if self.refs[slot].bucket_slot == r.bucket_slot
                && self.refs[slot].entry_idx == r.entry_idx
            {
                self.refs[slot] = r;
                return;
            }

            matches &= matches - 1;
        }

        let first_empty_offset = if mask.empty_mask != 0x00 {
            mask.empty_mask.trailing_zeros() as usize
        } else {
            PROBE_LIMIT
        };

        let first_tomb_offset = if mask.tomb_mask != 0x00 {
            let off = mask.tomb_mask.trailing_zeros() as usize;
            if off < first_empty_offset { Some(off) } else { None }
        } else {
            None
        };

        let insert_off = first_tomb_offset.unwrap_or(first_empty_offset);
        if insert_off < PROBE_LIMIT {
            let slot = (primary + insert_off) & (self.capacity - 0x01);
            let was_tomb = self.fingerprints[slot] == FP_TOMBSTONE;

            self.fingerprints[slot] = target_fp;
            self.refs[slot] = r;
            self.count += 0x01;

            if was_tomb {
                self.tombstones = self.tombstones.saturating_sub(0x01);
            }
        } else {
            let new_cap = self.capacity * 0x02;

            self.resize(new_cap);
            self.insert(hash, r);
        }
    }

    pub(crate) fn delete(&mut self, hash: u32, bucket_slot: u32, entry_idx: u8) -> bool {
        let target_fp = compute_fingerprint(hash);
        let primary = self.primary_slot(hash);
        let candidate_fps = self.load_16_fps(primary);

        let kernel = index_probe_kernel();
        let mask = unsafe { kernel(&candidate_fps, target_fp) };

        let mut matches = mask.match_mask;
        while matches != 0x00 {
            let off = matches.trailing_zeros() as usize;
            let slot = (primary + off) & (self.capacity - 0x01);

            if self.refs[slot].bucket_slot == bucket_slot && self.refs[slot].entry_idx == entry_idx
            {
                self.fingerprints[slot] = FP_TOMBSTONE;
                self.refs[slot] = IndexRef::default();
                self.count = self.count.saturating_sub(0x01);
                self.tombstones += 0x01;

                return true;
            }

            matches &= matches - 1;
        }

        false
    }

    fn resize(&mut self, new_capacity: usize) {
        let new_cap = new_capacity.max(MIN_CAPACITY).next_power_of_two();

        let mut new_fps = vec![FP_EMPTY; new_cap];
        let mut new_refs = vec![IndexRef::default(); new_cap];

        let kernel = index_probe_kernel();

        for i in 0..self.capacity {
            let fp = self.fingerprints[i];
            if fp < MIN_VALID_FP {
                continue;
            }

            let r = self.refs[i];
            let primary = (r.hash as usize) & (new_cap - 0x01);

            let mut placed = false;
            let mut p = primary;

            for _ in 0..0x08 {
                let mut buf = [0u16; PROBE_LIMIT];

                if p + PROBE_LIMIT <= new_cap {
                    buf.copy_from_slice(&new_fps[p..p + PROBE_LIMIT]);
                } else {
                    let head = new_cap - p;
                    buf[..head].copy_from_slice(&new_fps[p..new_cap]);
                    buf[head..].copy_from_slice(&new_fps[..PROBE_LIMIT - head]);
                }

                let mask = unsafe { kernel(&buf, fp) };
                if mask.empty_mask != 0x00 {
                    let off = mask.empty_mask.trailing_zeros() as usize;
                    let slot = (p + off) & (new_cap - 0x01);

                    new_fps[slot] = fp;
                    new_refs[slot] = r;
                    placed = true;

                    break;
                }

                p = (p + PROBE_LIMIT) & (new_cap - 0x01);
            }

            if !placed {
                for slot in 0..new_cap {
                    if new_fps[slot] == FP_EMPTY {
                        new_fps[slot] = fp;
                        new_refs[slot] = r;

                        break;
                    }
                }
            }
        }

        self.fingerprints = new_fps;
        self.refs = new_refs;
        self.capacity = new_cap;
        self.tombstones = 0x00;
    }

    #[inline(always)]
    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        self.count
    }

    #[inline(always)]
    #[allow(dead_code)]
    pub(crate) fn capacity(&self) -> usize {
        self.capacity
    }
}

#[derive(Debug)]
pub(crate) struct FpIndex {
    shards: Box<[RwLock<FpIndexShard>]>,
}

impl FpIndex {
    pub(crate) fn new(desired_capacity: usize) -> Self {
        let per_shard_cap = (desired_capacity / SHARD_COUNT).max(MIN_CAPACITY);

        let mut shards = Vec::with_capacity(SHARD_COUNT);
        for _ in 0..SHARD_COUNT {
            shards.push(RwLock::new(FpIndexShard::new(per_shard_cap)));
        }

        Self { shards: shards.into_boxed_slice() }
    }

    #[inline(always)]
    fn shard_idx(hash: u32) -> usize {
        ((hash >> 8) as usize) & (SHARD_COUNT - 0x01)
    }

    #[inline(always)]
    pub(crate) fn lookup<F>(&self, hash: u32, verify: F) -> LookupResult
    where
        F: FnMut(IndexRef) -> bool,
    {
        self.shards[Self::shard_idx(hash)]
            .read()
            .expect("fp index lock poisoned")
            .lookup(hash, verify)
    }

    #[inline(always)]
    pub(crate) fn insert(&self, hash: u32, r: IndexRef) {
        self.shards[Self::shard_idx(hash)].write().expect("fp index lock poisoned").insert(hash, r);
    }

    #[inline(always)]
    pub(crate) fn delete(&self, hash: u32, bucket_slot: u32, entry_idx: u8) -> bool {
        self.shards[Self::shard_idx(hash)].write().expect("fp index lock poisoned").delete(
            hash,
            bucket_slot,
            entry_idx,
        )
    }

    #[inline(always)]
    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        self.shards.iter().map(|s| s.read().expect("fp index lock poisoned").len()).sum()
    }

    #[inline(always)]
    #[allow(dead_code)]
    pub(crate) fn capacity(&self) -> usize {
        self.shards.iter().map(|s| s.read().expect("fp index lock poisoned").capacity()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_fp_index_crud() {
        let index = FpIndex::new(0x40);
        let hash = 0x1234_5678;
        let r = IndexRef {
            hash,
            bucket_slot: 0x2A,
            dbuf_slot: 0x10,
            entry_idx: 0x03,
            _pad: [0x00; 3],
            offset: 0,
        };

        let miss = index.lookup(hash, |_| true);
        assert_eq!(miss, LookupResult::DefiniteMiss);

        index.insert(hash, r);
        assert_eq!(index.len(), 0x01);

        let hit = index.lookup(hash, |found| found == r);
        assert_eq!(hit, LookupResult::Hit(r));

        let wrong_key = index.lookup(hash, |_| false);
        assert_eq!(wrong_key, LookupResult::DefiniteMiss);

        assert!(index.delete(hash, 0x2A, 0x03));
        assert_eq!(index.len(), 0x00);

        let after_del = index.lookup(hash, |found| found == r);
        assert_ne!(after_del, LookupResult::Hit(r));
    }

    #[test]
    fn ok_fp_index_dynamic_resizing() {
        let index = FpIndex::new(0x40);
        for i in 0x00..0x64 {
            let hash = 0x5442_0000 + i;
            let r = IndexRef {
                hash,
                bucket_slot: i,
                dbuf_slot: i + 0x10,
                entry_idx: (i % 0x40) as u8,
                _pad: [0x00; 3],
                offset: 0,
            };
            index.insert(hash, r);
        }

        assert_eq!(index.len(), 0x64);
        assert!(index.capacity() >= 0x80);

        for i in 0x00..0x64 {
            let hash = 0x5442_0000 + i;
            let expected_r = IndexRef {
                hash,
                bucket_slot: i,
                dbuf_slot: i + 0x10,
                entry_idx: (i % 0x40) as u8,
                _pad: [0x00; 3],
                offset: 0,
            };
            let res = index.lookup(hash, |r| r == expected_r);
            assert_eq!(res, LookupResult::Hit(expected_r));
        }
    }
}
