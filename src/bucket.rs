use crate::common::{
    ADJ_ARR_IDX_MASK, BUCKET_ENTRIES, BUCKET_PROBE_LIMIT, BUCKET_SIZE, BUCKET_SLOTS, HASH_EMPTY,
    HASH_SLICE_MASK, HASH_SLICES_PER_U32, HASH_TOMBSTONE, LIST_FLAG_BE, LIST_FLAG_MASK,
    MIN_VALID_HASH, Slot,
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
    Found(usize),
    Vacant(usize),
    Overflow { primary: usize, next_bucket: u32, candidate_slot: Option<usize> },
}

/// Ref: `DESIGN.md` §1, §3.2
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Bucket {
    pub(crate) hashes: [u32; BUCKET_ENTRIES],
    pub(crate) offsets: [u64; BUCKET_ENTRIES],
    pub(crate) next_bucket_indices: [u32; BUCKET_ENTRIES],
}

const _: () = assert!(core::mem::align_of::<Bucket>() == 8);
const _: () = assert!(core::mem::offset_of!(Bucket, hashes) == 0);
const _: () = assert!(core::mem::size_of::<Bucket>() == BUCKET_SIZE);
const _: () = assert!(core::mem::offset_of!(Bucket, offsets) == 0x0100);
const _: () = assert!(core::mem::size_of::<[u32; BUCKET_ENTRIES]>() == 0x0100);
const _: () = assert!(core::mem::size_of::<[u64; BUCKET_ENTRIES]>() == 0x0200);
const _: () = assert!(core::mem::size_of::<Bucket>() == BUCKET_SLOTS * Slot::SIZE);
const _: () = assert!(core::mem::offset_of!(Bucket, next_bucket_indices) == 0x0300);

const _: () = assert!(HASH_EMPTY == 0);
const _: () = assert!(HASH_TOMBSTONE == 1);
const _: () = assert!(MIN_VALID_HASH == 2);
const _: () = assert!(LIST_FLAG_MASK == 0x80);
const _: () = assert!(BUCKET_ENTRIES == 0x40);
const _: () = assert!(BUCKET_PROBE_LIMIT == 0x10);
const _: () = assert!(LIST_FLAG_BE == 0x8000_0000);
const _: () = assert!(BUCKET_SIZE % Slot::SIZE == 0);
const _: () = assert!(ADJ_ARR_IDX_MASK == 0x7FFF_FFFF);
const _: () = assert!(BUCKET_SIZE / Slot::SIZE == BUCKET_SLOTS);

const _: () = assert!(core::mem::align_of::<BucketEntry>() == 8);
const _: () = assert!(core::mem::size_of::<BucketEntry>() == 0x10);
const _: () = assert!(core::mem::offset_of!(BucketEntry, hash) == 0);
const _: () = assert!(core::mem::offset_of!(BucketEntry, offset) == 8);

const _: () = assert!(BucketEntry::empty().is_empty());
const _: () = assert!(!BucketEntry::empty().is_occupied());
const _: () = assert!(BucketEntry::tombstone().is_tombstone());
const _: () = assert!(!BucketEntry::tombstone().is_occupied());

const _: () = {
    let kv_test = BucketEntry::pack_kv(0x1234_5678, 0x0C, 0x0150, 0x0002_0000);
    assert!(kv_test.is_kv());
    assert!(!kv_test.is_list());
    assert!(kv_test.is_occupied());
    assert!(kv_test.klen() == 0x0C);
    assert!(kv_test.vlen() == 0x0150);
    assert!(kv_test.buf_idx() == 0x0002_0000);

    let list_test = BucketEntry::pack_list(0x9ABC_DEF0, 0x002A, 0x0005);
    assert!(!list_test.is_kv());
    assert!(list_test.is_list());
    assert!(list_test.is_occupied());
    assert!(list_test.adjarr_idx() == 0x002A);
    assert!(list_test.buf_idx() == 0x0005);
};

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
            hashes: [HASH_EMPTY; BUCKET_ENTRIES],
            offsets: [0; BUCKET_ENTRIES],
            next_bucket_indices: [0; BUCKET_ENTRIES],
        }
    }

    /// Ref: `DESIGN.md` §4.2
    #[inline(always)]
    pub(crate) const fn primary_slot(hash: u32, depth: u8) -> usize {
        let shift = 6 * ((depth as u32) % HASH_SLICES_PER_U32);
        ((hash >> shift) & HASH_SLICE_MASK) as usize
    }

    /// Ref: `DESIGN.md` §4.3
    #[inline(always)]
    #[allow(dead_code)]
    pub(crate) fn probe_indices(hash: u32, depth: u8) -> [usize; BUCKET_PROBE_LIMIT] {
        let s = Self::primary_slot(hash, depth);

        let mut i = 0x00;
        let mut indices = [0usize; BUCKET_PROBE_LIMIT];

        while i < BUCKET_PROBE_LIMIT {
            indices[i] = (s + i) & (BUCKET_ENTRIES - 0x01);
            i += 0x01;
        }

        indices
    }

    #[inline(always)]
    pub(crate) fn load_16_hashes(&self, primary: usize) -> [u32; BUCKET_PROBE_LIMIT] {
        let mut buf = [0u32; BUCKET_PROBE_LIMIT];

        if primary + BUCKET_PROBE_LIMIT <= BUCKET_ENTRIES {
            buf.copy_from_slice(&self.hashes[primary..primary + BUCKET_PROBE_LIMIT]);
        } else {
            let head = BUCKET_ENTRIES - primary;

            buf[..head].copy_from_slice(&self.hashes[primary..BUCKET_ENTRIES]);
            buf[head..].copy_from_slice(&self.hashes[..BUCKET_PROBE_LIMIT - head]);
        }

        buf
    }

    /// Ref: `DESIGN.md` §4.3, §4.4
    pub(crate) fn probe<F>(&self, hash: u32, depth: u8, mut key_match: F) -> ProbeResult
    where
        F: FnMut(usize) -> bool,
    {
        let primary = Self::primary_slot(hash, depth);
        let kernel = crate::probe::probe_kernel();

        let candidate_hashes = self.load_16_hashes(primary);
        let mask = unsafe { kernel(&candidate_hashes, hash) };

        let first_empty_offset = if mask.empty_mask != 0x00 {
            mask.empty_mask.trailing_zeros() as usize
        } else {
            BUCKET_PROBE_LIMIT
        };

        let first_tomb_offset = if mask.tomb_mask != 0x00 {
            let off = mask.tomb_mask.trailing_zeros() as usize;
            if off < first_empty_offset { Some(off) } else { None }
        } else {
            None
        };

        let valid_match_mask = if first_empty_offset < BUCKET_PROBE_LIMIT {
            mask.match_mask & ((1 << first_empty_offset) - 1)
        } else {
            mask.match_mask
        };

        let mut matches = valid_match_mask;
        while matches != 0x00 {
            let off = matches.trailing_zeros() as usize;
            let slot = (primary + off) & (BUCKET_ENTRIES - 0x01);

            if key_match(slot) {
                return ProbeResult::Found(slot);
            }

            matches &= matches - 1;
        }

        let candidate_slot = first_tomb_offset.map(|off| (primary + off) & (BUCKET_ENTRIES - 0x01));
        if first_empty_offset < BUCKET_PROBE_LIMIT {
            let empty_slot = (primary + first_empty_offset) & (BUCKET_ENTRIES - 0x01);

            return ProbeResult::Vacant(candidate_slot.unwrap_or(empty_slot));
        }

        let next_bucket = self.next_bucket_indices[primary];
        ProbeResult::Overflow { primary, next_bucket, candidate_slot }
    }

    #[inline(always)]
    pub(crate) fn entry(&self, idx: usize) -> BucketEntry {
        // sanity check
        debug_assert!(idx < BUCKET_ENTRIES);

        BucketEntry { hash: self.hashes[idx], offset: self.offsets[idx] }
    }

    #[allow(dead_code)]
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

    #[allow(dead_code)]
    #[inline(always)]
    pub(crate) fn clear_entry(&mut self, idx: usize) {
        // sanity check
        debug_assert!(idx < BUCKET_ENTRIES);

        self.hashes[idx] = HASH_EMPTY;
        self.offsets[idx] = 0;
    }

    #[allow(dead_code)]
    #[inline(always)]
    pub(crate) fn next_bucket(&self, primary: usize) -> u32 {
        // sanity check
        debug_assert!(primary < BUCKET_ENTRIES);

        self.next_bucket_indices[primary]
    }

    #[inline(always)]
    pub(crate) fn set_next_bucket(&mut self, primary: usize, next_slot: u32) {
        // sanity check
        debug_assert!(primary < BUCKET_ENTRIES);

        self.next_bucket_indices[primary] = next_slot;
    }

    #[inline]
    #[allow(dead_code)]
    pub(crate) fn occupied_count(&self) -> usize {
        self.hashes.iter().filter(|&&h| h >= MIN_VALID_HASH).count()
    }

    #[inline]
    #[allow(dead_code)]
    pub(crate) fn tombstone_count(&self) -> usize {
        self.hashes.iter().filter(|&&h| h == HASH_TOMBSTONE).count()
    }

    #[inline]
    #[allow(dead_code)]
    pub(crate) fn is_full(&self) -> bool {
        self.hashes.iter().all(|&h| h != HASH_EMPTY && h != HASH_TOMBSTONE)
    }

    #[inline(always)]
    pub(crate) fn as_slots(&self) -> &[Slot; BUCKET_SLOTS] {
        unsafe { &*(self as *const Self as *const [Slot; BUCKET_SLOTS]) }
    }

    #[allow(dead_code)]
    #[inline(always)]
    pub(crate) fn as_slots_mut(&mut self) -> &mut [Slot; BUCKET_SLOTS] {
        unsafe { &mut *(self as *mut Self as *mut [Slot; BUCKET_SLOTS]) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_bucket_layout_invariants() {
        assert_eq!(core::mem::size_of::<Bucket>(), 0x0400);
        assert_eq!(core::mem::align_of::<Bucket>(), 8);
        assert_eq!(core::mem::offset_of!(Bucket, hashes), 0);
        assert_eq!(core::mem::offset_of!(Bucket, offsets), 0x0100);
        assert_eq!(core::mem::offset_of!(Bucket, next_bucket_indices), 0x0300);
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
    fn ok_bucket_primary_slot_and_probing() {
        let hash = 0b001101_000101_000010_000001_000000_u32;
        assert_eq!(Bucket::primary_slot(hash, 0), 0);
        assert_eq!(Bucket::primary_slot(hash, 1), 1);
        assert_eq!(Bucket::primary_slot(hash, 2), 2);
        assert_eq!(Bucket::primary_slot(hash, 3), 5);
        assert_eq!(Bucket::primary_slot(hash, 4), 0x0D);
        assert_eq!(Bucket::primary_slot(hash, 5), 0);

        let indices = Bucket::probe_indices(hash, 0);
        assert_eq!(indices, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F]);

        let wrapped = Bucket::probe_indices(0x3F, 0);
        assert_eq!(wrapped, [0x3F, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E]);
    }

    #[test]
    fn ok_bucket_probe_workflow() {
        let mut b = Bucket::new();
        let hash = 0x5442_4658;
        let primary = Bucket::primary_slot(hash, 0);

        let res = b.probe(hash, 0, |_| false);
        assert_eq!(res, ProbeResult::Vacant(primary));

        b.set_kv(primary, hash, 4, 8, 1);
        let res_found = b.probe(hash, 0, |slot| slot == primary);
        assert_eq!(res_found, ProbeResult::Found(primary));

        b.mark_tombstone(primary);
        let res_tomb = b.probe(hash, 0, |_| false);
        assert_eq!(res_tomb, ProbeResult::Vacant(primary));
    }

    #[test]
    fn ok_bucket_overflow_resolution() {
        let mut b = Bucket::new();
        let hash = 0x2000;
        let primary = Bucket::primary_slot(hash, 0);
        let indices = Bucket::probe_indices(hash, 0);

        for (i, &idx) in indices.iter().enumerate() {
            b.set_kv(idx, (0x3000 + i) as u32, 4, 4, i as u32);
        }

        b.set_next_bucket(primary, 0x48);
        let res = b.probe(hash, 0, |_| false);
        assert_eq!(res, ProbeResult::Overflow { primary, next_bucket: 0x48, candidate_slot: None });

        b.mark_tombstone(indices[1]);
        let res_tomb = b.probe(hash, 0, |_| false);

        assert_eq!(
            res_tomb,
            ProbeResult::Overflow { primary, next_bucket: 0x48, candidate_slot: Some(indices[1]) }
        );
    }

    #[test]
    fn ok_bucket_probe_boundary_wrap_match() {
        let mut b = Bucket::new();
        let primary = 0x3C;

        let target_hash = 0x7777_883C;
        let match_slot = 0x05;

        for i in 0x00..0x10 {
            let slot = (primary + i) & 0x3F;

            if slot == match_slot {
                b.set_kv(slot, target_hash, 4, 4, slot as u32);
            } else {
                b.set_kv(slot, 0x1000 + slot as u32, 4, 4, slot as u32);
            }
        }

        let res = b.probe(target_hash, 0, |s| s == match_slot);
        assert_eq!(res, ProbeResult::Found(match_slot));
    }

    #[test]
    fn ok_bucket_probe_stops_at_first_empty() {
        let mut b = Bucket::new();
        let target_hash = 0x4444_5540;

        b.set_kv(0, 0x1000, 4, 4, 0);
        b.set_kv(1, 0x1001, 4, 4, 1);
        b.set_kv(4, target_hash, 4, 4, 4);

        let res = b.probe(target_hash, 0, |s| s == 4);
        assert_eq!(res, ProbeResult::Vacant(2));
    }

    #[test]
    fn ok_bucket_probe_tombstone_reclaim_over_later_empty() {
        let mut b = Bucket::new();
        let target_hash = 0x6666_7740;

        b.set_kv(0, 0x1000, 4, 4, 0);
        b.mark_tombstone(1);
        b.set_kv(2, 0x1002, 4, 4, 2);

        let res = b.probe(target_hash, 0, |_| false);
        assert_eq!(res, ProbeResult::Vacant(1));
    }

    #[test]
    fn ok_bucket_probe_match_at_slot_15() {
        let mut b = Bucket::new();
        let target_hash = 0x8888_9940;

        for i in 0x00..0x0F {
            b.set_kv(i, (0x2000 + i) as u32, 4, 4, i as u32);
        }

        b.set_kv(0x0F, target_hash, 4, 4, 0x0F);
        let res = b.probe(target_hash, 0, |s| s == 0x0F);

        assert_eq!(res, ProbeResult::Found(0x0F));
    }

    #[test]
    fn ok_bucket_as_slots() {
        let mut b = Bucket::new();
        b.set_kv(0, 0x5000, 4, 4, 1);

        let slots = b.as_slots();
        assert_eq!(slots.len(), 8);
        assert_eq!(b.as_slots_mut().len(), 8);
    }
}
