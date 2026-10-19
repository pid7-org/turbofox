//! Hash Bucket and BucketEntry structures (0x0100 B / 2 Slots)
//!
//! Ref: `DESIGN.md` §3.2, §3.4, §4.3, §5.2

use crate::hash::{HASH_EMPTY, HASH_TOMBSTONE, MIN_VALID_HASH};
use crate::slot::Slot;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) struct BucketEntry {
    klen: u16,
    vlen: u16,
    buf_idx: u32,
    hash: u32,
}

const _: () = assert!(core::mem::size_of::<BucketEntry>() == 0x0C);

impl BucketEntry {
    pub(crate) const LIST_FLAG: u32 = 0x8000_0000;
    pub(crate) const BUF_IDX_MASK: u32 = 0x7FFF_FFFF;

    #[inline(always)]
    pub(crate) const fn empty() -> Self {
        Self { klen: 0, vlen: 0, buf_idx: 0, hash: HASH_EMPTY }
    }

    #[inline(always)]
    pub(crate) const fn tombstone() -> Self {
        Self { klen: 0, vlen: 0, buf_idx: 0, hash: HASH_TOMBSTONE }
    }

    #[inline(always)]
    pub(crate) const fn klen(&self) -> u16 {
        self.klen
    }

    #[inline(always)]
    pub(crate) const fn vlen(&self) -> u16 {
        self.vlen
    }

    #[inline(always)]
    pub(crate) const fn hash(&self) -> u32 {
        self.hash
    }

    #[inline(always)]
    pub(crate) fn is_empty(&self) -> bool {
        self.hash == HASH_EMPTY
    }

    #[inline(always)]
    pub(crate) fn is_tombstone(&self) -> bool {
        self.hash == HASH_TOMBSTONE
    }

    #[inline(always)]
    pub(crate) fn is_occupied(&self) -> bool {
        self.hash >= MIN_VALID_HASH
    }

    #[inline(always)]
    pub(crate) fn is_list(&self) -> bool {
        (self.buf_idx & Self::LIST_FLAG) != 0
    }

    #[inline(always)]
    pub(crate) fn buffer_index(&self) -> u32 {
        self.buf_idx & Self::BUF_IDX_MASK
    }

    #[inline(always)]
    pub(crate) fn set_buffer_index(&mut self, idx: u32, is_list: bool) {
        let flag = if is_list { Self::LIST_FLAG } else { 0 };
        self.buf_idx = flag | (idx & Self::BUF_IDX_MASK);
    }

    #[inline(always)]
    pub(crate) fn set_kv(&mut self, klen: u16, vlen: u16, buf_idx: u32, hash: u32) {
        self.klen = klen;
        self.vlen = vlen;
        self.hash = hash;
        self.set_buffer_index(buf_idx, false);
    }

    #[inline(always)]
    pub(crate) fn set_list(&mut self, klen: u16, head_adj: u16, buf_idx: u32, hash: u32) {
        self.klen = klen;
        self.vlen = head_adj;
        self.hash = hash;
        self.set_buffer_index(buf_idx, true);
    }

    #[inline(always)]
    pub(crate) fn mark_tombstone(&mut self) {
        self.hash = HASH_TOMBSTONE;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProbeResult {
    Found(usize),
    Vacant(usize),
    Full,
}

#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Bucket {
    header: u32,
    entries: [BucketEntry; 0x15],
}

const _: () = assert!(core::mem::align_of::<Bucket>() == 8);
const _: () = assert!(core::mem::size_of::<Bucket>() == 0x0100);

impl Default for Bucket {
    #[inline(always)]
    fn default() -> Self {
        Self::new()
    }
}

impl Bucket {
    pub(crate) const ENTRY_COUNT: usize = 0x15;
    pub(crate) const BUCKET_BIT: u32 = 0x8000_0000;

    #[inline(always)]
    pub(crate) const fn new() -> Self {
        Self { header: Self::BUCKET_BIT, entries: [BucketEntry::empty(); Self::ENTRY_COUNT] }
    }

    #[inline(always)]
    pub(crate) fn is_bucket(&self) -> bool {
        (self.header & Self::BUCKET_BIT) != 0
    }

    #[inline(always)]
    pub(crate) const fn initial_slot(hash: u32, depth: u8) -> usize {
        let shift = match (6 * (depth as u32)).checked_rem(0x20) {
            Some(s) => s,
            None => 0,
        };
        ((hash >> shift) % (Self::ENTRY_COUNT as u32)) as usize
    }

    #[inline(always)]
    pub(crate) fn probe_indices(hash: u32, depth: u8) -> impl Iterator<Item = usize> {
        let start = Self::initial_slot(hash, depth);
        (0..Self::ENTRY_COUNT).map(move |i| (start + i) % Self::ENTRY_COUNT)
    }

    #[inline(always)]
    pub(crate) fn probe<F>(&self, hash: u32, depth: u8, mut key_match: F) -> ProbeResult
    where
        F: FnMut(&BucketEntry) -> bool,
    {
        let mut first_tombstone = None;

        for slot in Self::probe_indices(hash, depth) {
            let entry = &self.entries[slot];
            if entry.is_empty() {
                return ProbeResult::Vacant(first_tombstone.unwrap_or(slot));
            }
            if entry.is_tombstone() {
                if first_tombstone.is_none() {
                    first_tombstone = Some(slot);
                }
                continue;
            }
            if entry.hash == hash && key_match(entry) {
                return ProbeResult::Found(slot);
            }
        }

        match first_tombstone {
            Some(slot) => ProbeResult::Vacant(slot),
            None => ProbeResult::Full,
        }
    }

    #[inline(always)]
    pub(crate) fn is_full(&self) -> bool {
        self.entries.iter().all(|e| e.is_occupied())
    }

    #[inline(always)]
    pub(crate) fn entry(&self, idx: usize) -> &BucketEntry {
        &self.entries[idx]
    }

    #[inline(always)]
    pub(crate) fn entry_mut(&mut self, idx: usize) -> &mut BucketEntry {
        &mut self.entries[idx]
    }

    #[inline(always)]
    pub(crate) fn route_entries_for_split(
        &self,
        next_depth: u8,
    ) -> impl Iterator<Item = (usize, BucketEntry)> + '_ {
        let shift = (6 * (next_depth as u32)) % 0x20;
        self.entries.iter().copied().filter(|e| e.is_occupied()).map(move |e| {
            let child_idx = ((e.hash >> shift) & 0x3F) as usize;
            (child_idx, e)
        })
    }

    #[inline(always)]
    pub(crate) fn as_slots(&self) -> &[Slot; 2] {
        unsafe { &*(self as *const Self as *const [Slot; 2]) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_bucket_layout_invariants() {
        assert_eq!(core::mem::size_of::<BucketEntry>(), 0x0C);
        assert_eq!(core::mem::size_of::<Bucket>(), 0x0100);
        assert_eq!(core::mem::align_of::<Bucket>(), 8);
    }

    #[test]
    fn ok_bucket_entry_offsets() {
        assert_eq!(core::mem::offset_of!(BucketEntry, klen), 0);
        assert_eq!(core::mem::offset_of!(BucketEntry, vlen), 2);
        assert_eq!(core::mem::offset_of!(BucketEntry, buf_idx), 4);
        assert_eq!(core::mem::offset_of!(BucketEntry, hash), 8);
    }

    #[test]
    fn ok_bucket_discriminator_and_flags() {
        let mut b = Bucket::new();
        assert!(b.is_bucket());

        let mut entry = BucketEntry::default();
        assert!(entry.is_empty());
        assert!(!entry.is_tombstone());
        assert!(!entry.is_occupied());

        entry.hash = 0x1234_5678;
        assert!(entry.is_occupied());

        entry.set_buffer_index(0x2A, true);
        assert!(entry.is_list());
        assert_eq!(entry.buffer_index(), 0x2A);

        entry.set_buffer_index(0x2A, false);
        assert!(!entry.is_list());
        assert_eq!(entry.buffer_index(), 0x2A);

        b.entries[0] = entry;
        assert_eq!(b.entries[0].buffer_index(), 0x2A);
    }

    #[test]
    fn ok_bucket_entry_accessors_and_mutators() {
        let mut entry = BucketEntry::empty();
        assert_eq!(entry.klen(), 0);
        assert_eq!(entry.vlen(), 0);
        assert_eq!(entry.hash(), 0);

        entry.set_kv(0x0A, 0x20, 0x05, 0x9000);
        assert_eq!(entry.klen(), 0x0A);
        assert_eq!(entry.vlen(), 0x20);
        assert_eq!(entry.hash(), 0x9000);
        assert_eq!(entry.buffer_index(), 0x05);
        assert!(!entry.is_list());

        entry.set_list(4, 8, 0x12, 0x9001);
        assert_eq!(entry.klen(), 4);
        assert_eq!(entry.vlen(), 8);
        assert_eq!(entry.hash(), 0x9001);
        assert_eq!(entry.buffer_index(), 0x12);
        assert!(entry.is_list());

        entry.mark_tombstone();
        assert!(entry.is_tombstone());
        assert!(!entry.is_empty());
        assert!(!entry.is_occupied());
    }

    #[test]
    fn ok_bucket_initial_slot_and_indices() {
        let hash = 0x5442_4658;
        let slot_d0 = Bucket::initial_slot(hash, 0);
        assert_eq!(slot_d0, (hash % 0x15) as usize);

        let indices: Vec<usize> = Bucket::probe_indices(hash, 0).collect();
        assert_eq!(indices.len(), 0x15);
        assert_eq!(indices[0], slot_d0);
        assert_eq!(indices[1], (slot_d0 + 1) % 0x15);
    }

    #[test]
    fn ok_bucket_probe_resolution() {
        let mut b = Bucket::new();
        let depth = 0;
        let hash = 0x2000;

        let res = b.probe(hash, depth, |_| false);
        let start_slot = Bucket::initial_slot(hash, depth);
        assert_eq!(res, ProbeResult::Vacant(start_slot));

        b.entry_mut(start_slot).set_kv(4, 4, 1, hash);
        let res2 = b.probe(hash, depth, |e| e.klen() == 4);
        assert_eq!(res2, ProbeResult::Found(start_slot));

        let res_miss = b.probe(hash, depth, |_| false);
        let next_slot = (start_slot + 1) % 0x15;
        assert_eq!(res_miss, ProbeResult::Vacant(next_slot));

        b.entry_mut(start_slot).mark_tombstone();
        let res_tomb = b.probe(hash, depth, |_| false);
        assert_eq!(res_tomb, ProbeResult::Vacant(start_slot));
    }

    #[test]
    fn ok_bucket_probe_full() {
        let mut b = Bucket::new();
        for i in 0..0x15 {
            b.entry_mut(i).set_kv(4, 4, i as u32, (0x100 + i) as u32);
        }
        assert!(b.is_full());

        let res = b.probe(0x9999, 0, |_| false);
        assert_eq!(res, ProbeResult::Full);
    }

    #[test]
    fn ok_bucket_split_routing() {
        let mut b2 = Bucket::new();
        b2.entry_mut(0).set_kv(4, 4, 1, 0b000101_000000);
        b2.entry_mut(1).set_kv(4, 4, 2, 0b001010_000000);

        let routes: Vec<(usize, BucketEntry)> = b2.route_entries_for_split(1).collect();
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].0, 0b000101);
        assert_eq!(routes[1].0, 0b001010);
    }
}
