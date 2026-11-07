use crate::common::{BUCKET_ENTRIES, BUCKET_SIZE, BUCKET_SLOTS, HASH_EMPTY, HASH_TOMBSTONE, Slot};
use crate::hash::LIST_HASH_BIT;

/// Ref: `DESIGN.md` §3.3
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BucketEntry {
    hash: u32,
    buf_idx: u32,
}

impl BucketEntry {
    #[inline(always)]
    pub(crate) const fn new(hash: u32, buf_idx: u32) -> Self {
        Self { hash, buf_idx }
    }

    #[inline(always)]
    pub(crate) const fn hash(&self) -> u32 {
        self.hash
    }

    #[inline(always)]
    pub(crate) const fn buf_idx(&self) -> u32 {
        self.buf_idx
    }

    #[inline(always)]
    pub(crate) const fn is_list(&self) -> bool {
        (self.hash & LIST_HASH_BIT) != 0
    }
}

/// Ref: `DESIGN.md` §4.3
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProbeResult {
    Full,
    Found(usize),
    Vacant(usize),
}

/// Ref: `DESIGN.md` §1, §3.3
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Bucket {
    hashes: [u32; BUCKET_ENTRIES],
    offsets: [u32; BUCKET_ENTRIES],
}

const _: () = assert!(core::mem::align_of::<Bucket>() == 8);
const _: () = assert!(core::mem::size_of::<Bucket>() == BUCKET_SIZE);
const _: () = assert!(core::mem::offset_of!(Bucket, hashes) == 0);
const _: () = assert!(core::mem::offset_of!(Bucket, offsets) == 0x80);
const _: () = assert!(core::mem::size_of::<[u32; BUCKET_ENTRIES]>() == 0x80);
const _: () = assert!(core::mem::size_of::<[u32; BUCKET_ENTRIES]>() == 0x80);
const _: () = assert!(core::mem::size_of::<Bucket>() == BUCKET_SLOTS * Slot::SIZE);

const _: () = assert!(HASH_EMPTY == 0);
const _: () = assert!(HASH_TOMBSTONE == 1);
const _: () = assert!(BUCKET_ENTRIES == 0x20);
const _: () = assert!(BUCKET_SIZE % Slot::SIZE == 0);

const _: () = assert!(core::mem::align_of::<BucketEntry>() == 4);
const _: () = assert!(core::mem::size_of::<BucketEntry>() == 8);
const _: () = assert!(core::mem::offset_of!(BucketEntry, hash) == 0);
const _: () = assert!(core::mem::offset_of!(BucketEntry, buf_idx) == 4);

impl Bucket {
    #[inline(always)]
    pub(crate) const fn new() -> Self {
        Self { hashes: [HASH_EMPTY; BUCKET_ENTRIES], offsets: [0; BUCKET_ENTRIES] }
    }

    #[inline(always)]
    pub(crate) fn probe_match<F>(&self, hash: u32, mut key_match: F) -> Option<usize>
    where
        F: FnMut(usize) -> bool,
    {
        let mut matches = crate::probe::probe_match(&self.hashes, hash);
        while matches != 0x00 {
            let slot = matches.trailing_zeros() as usize;

            if slot < BUCKET_ENTRIES && key_match(slot) {
                return Some(slot);
            }

            matches &= matches - 1;
        }

        None
    }

    pub(crate) fn probe<F>(&self, hash: u32, mut key_match: F) -> ProbeResult
    where
        F: FnMut(usize) -> bool,
    {
        let mask = crate::probe::probe_full(&self.hashes, hash);

        let first_empty = if mask.empty_mask != 0x00 {
            let slot = mask.empty_mask.trailing_zeros() as usize;
            if slot < BUCKET_ENTRIES { Some(slot) } else { None }
        } else {
            None
        };

        let valid_match_mask = if let Some(empty_idx) = first_empty {
            mask.match_mask & ((0x01u32 << empty_idx) - 0x01)
        } else {
            mask.match_mask
        };

        let mut matches = valid_match_mask;
        while matches != 0x00 {
            let slot = matches.trailing_zeros() as usize;
            if slot < BUCKET_ENTRIES && key_match(slot) {
                return ProbeResult::Found(slot);
            }

            matches &= matches - 1;
        }

        let first_tomb = if mask.tomb_mask != 0x00 {
            let slot = mask.tomb_mask.trailing_zeros() as usize;
            if slot < BUCKET_ENTRIES { Some(slot) } else { None }
        } else {
            None
        };

        if let Some(vacant) = first_tomb.or(first_empty) {
            ProbeResult::Vacant(vacant)
        } else {
            ProbeResult::Full
        }
    }

    #[inline(always)]
    pub(crate) fn entry(&self, idx: usize) -> BucketEntry {
        // sanity check
        debug_assert!(idx < BUCKET_ENTRIES);

        BucketEntry { hash: self.hashes[idx], buf_idx: self.offsets[idx] }
    }

    #[inline(always)]
    pub(crate) fn set_entry(&mut self, idx: usize, entry: BucketEntry) {
        // sanity check
        debug_assert!(idx < BUCKET_ENTRIES);

        self.hashes[idx] = entry.hash;
        self.offsets[idx] = entry.buf_idx;
    }

    #[inline(always)]
    pub(crate) fn set_kv(&mut self, idx: usize, hash: u32, buf_idx: u32) {
        self.set_entry(idx, BucketEntry::new(hash, buf_idx));
    }

    #[inline(always)]
    pub(crate) fn set_list(&mut self, idx: usize, hash: u32, buf_idx: u32) {
        self.set_entry(idx, BucketEntry::new(hash, buf_idx));
    }

    #[inline(always)]
    pub(crate) fn mark_tombstone(&mut self, idx: usize) {
        // sanity check
        debug_assert!(idx < BUCKET_ENTRIES);

        self.hashes[idx] = HASH_TOMBSTONE;
    }

    #[inline(always)]
    pub(crate) fn as_slots(&self) -> &[Slot; BUCKET_SLOTS] {
        unsafe { &*core::ptr::from_ref(self).cast() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_bucket_layout_invariants() {
        assert_eq!(core::mem::align_of::<Bucket>(), 8);
        assert_eq!(core::mem::size_of::<Bucket>(), 0x0100);
        assert_eq!(core::mem::offset_of!(Bucket, hashes), 0);
        assert_eq!(core::mem::offset_of!(Bucket, offsets), 0x80);
        assert_eq!(core::mem::size_of::<BucketEntry>(), 8);
        assert_eq!(core::mem::align_of::<BucketEntry>(), 4);
    }

    #[test]
    fn ok_bucket_entry_encoding() {
        let entry = BucketEntry::new(0x1234_5678, 0x0002_0000);
        assert!(!entry.is_list());
        assert_eq!(entry.hash(), 0x1234_5678);
        assert_eq!(entry.buf_idx(), 0x0002_0000);

        let list_entry = BucketEntry::new(0x9ABC_DEF0, 0x0005);
        assert!(list_entry.is_list());
        assert_eq!(list_entry.hash(), 0x9ABC_DEF0);
        assert_eq!(list_entry.buf_idx(), 0x0005);
    }

    #[test]
    fn ok_bucket_probe_workflow() {
        let mut b = Bucket::new();
        let hash = 0x5442_4658;

        let res = b.probe(hash, |_| false);
        assert_eq!(res, ProbeResult::Vacant(0));

        b.set_kv(0, hash, 1);
        let res_found = b.probe(hash, |slot| slot == 0);
        assert_eq!(res_found, ProbeResult::Found(0));

        b.mark_tombstone(0);
        let res_tomb = b.probe(hash, |_| false);
        assert_eq!(res_tomb, ProbeResult::Vacant(0));

        let match_found = b.probe_match(hash, |_| true);
        assert_eq!(match_found, None);

        b.set_kv(1, hash, 2);
        let match_hit = b.probe_match(hash, |slot| slot == 1);
        assert_eq!(match_hit, Some(1));
    }

    #[test]
    fn ok_bucket_full_resolution() {
        let mut b = Bucket::new();
        for i in 0..BUCKET_ENTRIES {
            b.set_kv(i, (0x3000 + i) as u32, i as u32);
        }

        let res = b.probe(0x9999, |_| false);
        assert_eq!(res, ProbeResult::Full);

        b.mark_tombstone(3);
        let res_vacant = b.probe(0x9999, |_| false);
        assert_eq!(res_vacant, ProbeResult::Vacant(3));
    }

    #[test]
    fn ok_bucket_as_slots() {
        let mut b = Bucket::new();
        b.set_kv(0, 0x5000, 1);

        assert_eq!(b.as_slots()[0].len(), 0x40);
        assert_eq!(b.as_slots().len(), 4);
    }
}
