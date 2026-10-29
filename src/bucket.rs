use crate::common::{
    ADJ_ARR_IDX_MASK, BUCKET_ENTRIES, BUCKET_FLAG, BUCKET_SIZE, BUCKET_SLOTS, HASH_EMPTY,
    HASH_TOMBSTONE, LIST_FLAG_BE, LIST_FLAG_MASK, MIN_VALID_HASH, Slot,
};

/// Ref: `DESIGN.md` §3.2
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) struct BucketEntry {
    hash: u32,
    offset: u64,
}

#[allow(dead_code)]
impl BucketEntry {
    #[inline(always)]
    pub(crate) const fn empty() -> Self {
        Self { hash: HASH_EMPTY, offset: 0 }
    }

    #[inline(always)]
    pub(crate) const fn tombstone() -> Self {
        Self { hash: HASH_TOMBSTONE, offset: 0 }
    }

    #[inline(always)]
    pub(crate) const fn hash(&self) -> u32 {
        self.hash
    }

    #[inline(always)]
    pub(crate) const fn offset(&self) -> u64 {
        self.offset
    }

    #[inline(always)]
    pub(crate) const fn is_empty(&self) -> bool {
        self.hash == HASH_EMPTY
    }

    #[inline(always)]
    pub(crate) const fn is_tombstone(&self) -> bool {
        self.hash == HASH_TOMBSTONE
    }

    #[inline(always)]
    pub(crate) const fn is_occupied(&self) -> bool {
        self.hash >= MIN_VALID_HASH
    }

    #[inline(always)]
    pub(crate) const fn is_list(&self) -> bool {
        (self.offset.to_ne_bytes()[0] & LIST_FLAG_MASK) != 0
    }

    #[inline(always)]
    pub(crate) const fn is_kv(&self) -> bool {
        self.is_occupied() && !self.is_list()
    }

    #[inline(always)]
    pub(crate) const fn klen(&self) -> u8 {
        self.offset.to_ne_bytes()[1]
    }

    #[inline(always)]
    pub(crate) const fn vlen(&self) -> u16 {
        let b = self.offset.to_ne_bytes();
        u16::from_le_bytes([b[2], b[3]])
    }

    #[inline(always)]
    pub(crate) const fn buf_idx(&self) -> u32 {
        let b = self.offset.to_ne_bytes();
        u32::from_le_bytes([b[4], b[5], b[6], b[7]])
    }

    #[inline(always)]
    pub(crate) const fn adjarr_idx(&self) -> u32 {
        let b = self.offset.to_ne_bytes();
        let be = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);

        be & ADJ_ARR_IDX_MASK
    }

    #[inline(always)]
    pub(crate) const fn pack_kv(hash: u32, klen: u8, vlen: u16, buf_idx: u32) -> Self {
        Self { hash, offset: encode_kv_offset(klen, vlen, buf_idx) }
    }

    #[inline(always)]
    pub(crate) const fn pack_list(hash: u32, adjarr_idx: u32, buf_idx: u32) -> Self {
        Self { hash, offset: encode_list_offset(adjarr_idx, buf_idx) }
    }
}

/// Ref: `DESIGN.md` §3.2
#[inline(always)]
pub(crate) const fn encode_kv_offset(klen: u8, vlen: u16, buf_idx: u32) -> u64 {
    let mut b = [0u8; 8];
    b[0] = 0;
    b[1] = klen;

    let v_bytes = vlen.to_le_bytes();
    b[2] = v_bytes[0];
    b[3] = v_bytes[1];

    let buf_bytes = buf_idx.to_le_bytes();
    b[4] = buf_bytes[0];
    b[5] = buf_bytes[1];
    b[6] = buf_bytes[2];
    b[7] = buf_bytes[3];

    u64::from_ne_bytes(b)
}

/// Ref: `DESIGN.md` §3.2
#[inline(always)]
pub(crate) const fn encode_list_offset(adjarr_idx: u32, buf_idx: u32) -> u64 {
    let mut b = [0u8; 8];
    let be = LIST_FLAG_BE | (adjarr_idx & ADJ_ARR_IDX_MASK);

    let be_bytes = be.to_be_bytes();
    b[0] = be_bytes[0];
    b[1] = be_bytes[1];
    b[2] = be_bytes[2];
    b[3] = be_bytes[3];

    let buf_bytes = buf_idx.to_le_bytes();
    b[4] = buf_bytes[0];
    b[5] = buf_bytes[1];
    b[6] = buf_bytes[2];
    b[7] = buf_bytes[3];

    u64::from_ne_bytes(b)
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
    flag: u32,
    hashes: [u32; BUCKET_ENTRIES],
    offsets: [u64; BUCKET_ENTRIES],
}

const _: () = assert!(core::mem::align_of::<Bucket>() == 8);
const _: () = assert!(core::mem::size_of::<Bucket>() == BUCKET_SIZE);
const _: () = assert!(core::mem::offset_of!(Bucket, flag) == 0);
const _: () = assert!(core::mem::offset_of!(Bucket, hashes) == 4);
const _: () = assert!(core::mem::offset_of!(Bucket, offsets) == 0x58);
const _: () = assert!(core::mem::size_of::<[u32; BUCKET_ENTRIES]>() == 84);
const _: () = assert!(core::mem::size_of::<[u64; BUCKET_ENTRIES]>() == 168);
const _: () = assert!(core::mem::size_of::<Bucket>() == BUCKET_SLOTS * Slot::SIZE);

const _: () = assert!(HASH_EMPTY == 0);
const _: () = assert!(HASH_TOMBSTONE == 1);
const _: () = assert!(MIN_VALID_HASH == 2);
const _: () = assert!(BUCKET_ENTRIES == 21);
const _: () = assert!(LIST_FLAG_MASK == 0x80);
const _: () = assert!(LIST_FLAG_BE == 0x8000_0000);
const _: () = assert!(BUCKET_SIZE % Slot::SIZE == 0);
const _: () = assert!(ADJ_ARR_IDX_MASK == 0x7FFF_FFFF);

const _: () = assert!(core::mem::align_of::<BucketEntry>() == 8);
const _: () = assert!(core::mem::size_of::<BucketEntry>() == 0x10);
const _: () = assert!(core::mem::offset_of!(BucketEntry, hash) == 0);
const _: () = assert!(core::mem::offset_of!(BucketEntry, offset) == 8);

impl Default for Bucket {
    #[inline(always)]
    fn default() -> Self {
        Self::new()
    }
}

impl Bucket {
    #[inline(always)]
    pub(crate) const fn new() -> Self {
        Self {
            flag: BUCKET_FLAG,
            hashes: [HASH_EMPTY; BUCKET_ENTRIES],
            offsets: [0; BUCKET_ENTRIES],
        }
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

        BucketEntry { hash: self.hashes[idx], offset: self.offsets[idx] }
    }

    #[inline(always)]
    pub(crate) fn set_entry(&mut self, idx: usize, entry: BucketEntry) {
        // sanity check
        debug_assert!(idx < BUCKET_ENTRIES);

        self.hashes[idx] = entry.hash;
        self.offsets[idx] = entry.offset;
    }

    #[inline(always)]
    pub(crate) fn set_kv(&mut self, idx: usize, hash: u32, klen: u8, vlen: u16, buf_idx: u32) {
        // sanity check
        debug_assert!(idx < BUCKET_ENTRIES);

        self.hashes[idx] = hash;
        self.offsets[idx] = encode_kv_offset(klen, vlen, buf_idx);
    }

    #[inline(always)]
    pub(crate) fn set_list(&mut self, idx: usize, hash: u32, adjarr_idx: u32, buf_idx: u32) {
        // sanity check
        debug_assert!(idx < BUCKET_ENTRIES);

        self.hashes[idx] = hash;
        self.offsets[idx] = encode_list_offset(adjarr_idx, buf_idx);
    }

    #[inline(always)]
    pub(crate) fn mark_tombstone(&mut self, idx: usize) {
        // sanity check
        debug_assert!(idx < BUCKET_ENTRIES);

        self.hashes[idx] = HASH_TOMBSTONE;
    }

    #[inline]
    #[allow(dead_code)]
    pub(crate) fn is_full(&self) -> bool {
        self.hashes.iter().all(|&h| h >= MIN_VALID_HASH)
    }

    #[inline(always)]
    #[allow(dead_code)]
    pub(crate) fn as_slot(&self) -> &Slot {
        unsafe { &*core::ptr::from_ref(self).cast() }
    }

    #[allow(dead_code)]
    #[inline(always)]
    pub(crate) fn as_slot_mut(&mut self) -> &mut Slot {
        unsafe { &mut *core::ptr::from_mut(self).cast() }
    }

    #[inline(always)]
    pub(crate) fn as_slots(&self) -> &[Slot; BUCKET_SLOTS] {
        unsafe { &*core::ptr::from_ref(self).cast() }
    }

    #[allow(dead_code)]
    #[inline(always)]
    pub(crate) fn as_slots_mut(&mut self) -> &mut [Slot; BUCKET_SLOTS] {
        unsafe { &mut *core::ptr::from_mut(self).cast() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_bucket_layout_invariants() {
        assert_eq!(core::mem::align_of::<Bucket>(), 8);
        assert_eq!(core::mem::size_of::<Bucket>(), 0x0100);
        assert_eq!(core::mem::offset_of!(Bucket, flag), 0);
        assert_eq!(core::mem::offset_of!(Bucket, hashes), 4);
        assert_eq!(core::mem::offset_of!(Bucket, offsets), 0x58);
    }

    #[test]
    fn ok_bucket_entry_kv_encoding() {
        let entry = BucketEntry::pack_kv(0x1234_5678, 0x0C, 0x0150, 0x0002_0000);
        assert!(entry.is_occupied());
        assert!(!entry.is_empty());
        assert!(!entry.is_tombstone());
        assert!(entry.is_kv());
        assert!(!entry.is_list());
        assert_eq!(entry.klen(), 0x0C);
        assert_eq!(entry.vlen(), 0x0150);
        assert_eq!(entry.buf_idx(), 0x0002_0000);
    }

    #[test]
    fn ok_bucket_entry_list_encoding() {
        let entry = BucketEntry::pack_list(0x9ABC_DEF0, 0x002A, 0x0005);
        assert!(entry.is_occupied());
        assert!(entry.is_list());
        assert!(!entry.is_kv());
        assert_eq!(entry.adjarr_idx(), 0x002A);
        assert_eq!(entry.buf_idx(), 0x0005);
    }

    #[test]
    fn ok_bucket_probe_workflow() {
        let mut b = Bucket::new();
        let hash = 0x5442_4658;

        let res = b.probe(hash, |_| false);
        assert_eq!(res, ProbeResult::Vacant(0));

        b.set_kv(0, hash, 4, 8, 1);
        let res_found = b.probe(hash, |slot| slot == 0);
        assert_eq!(res_found, ProbeResult::Found(0));

        b.mark_tombstone(0);
        let res_tomb = b.probe(hash, |_| false);
        assert_eq!(res_tomb, ProbeResult::Vacant(0));

        let match_found = b.probe_match(hash, |_| true);
        assert_eq!(match_found, None);

        b.set_kv(1, hash, 4, 8, 2);
        let match_hit = b.probe_match(hash, |slot| slot == 1);
        assert_eq!(match_hit, Some(1));
    }

    #[test]
    fn ok_bucket_full_resolution() {
        let mut b = Bucket::new();
        for i in 0..BUCKET_ENTRIES {
            b.set_kv(i, (0x3000 + i) as u32, 4, 4, i as u32);
        }

        assert!(b.is_full());
        let res = b.probe(0x9999, |_| false);
        assert_eq!(res, ProbeResult::Full);

        b.mark_tombstone(3);
        assert!(!b.is_full());
        let res_vacant = b.probe(0x9999, |_| false);
        assert_eq!(res_vacant, ProbeResult::Vacant(3));
    }

    #[test]
    fn ok_bucket_as_slots() {
        let mut b = Bucket::new();
        b.set_kv(0, 0x5000, 4, 4, 1);

        assert_eq!(b.as_slot().len(), 0x40);
        assert_eq!(b.as_slots().len(), 4);
        assert_eq!(b.as_slots_mut().len(), 4);
    }
}
