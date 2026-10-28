#![allow(dead_code)]

use core::ops::{Deref, DerefMut};

/// Ref: `DESIGN.md` §3.1
pub(crate) const MAGIC: [u8; 4] = *b"TBFX";

/// Ref: `DESIGN.md` §4.1
pub(crate) const SEED: u32 = 0x5442_4658;

/// Ref: `DESIGN.md` §8.2
pub(crate) const MODULE_ID: u8 = 0x46;

/// Ref: `DESIGN.md` §8.2
pub(crate) const DB_NAME_TAG: [u8; 4] = *b"tbfx";

/// Ref: `DESIGN.md` §8.2
#[allow(dead_code)]
pub(crate) const DEFAULT_VERSION: u32 = 3;

/// Ref: `DESIGN.md` §3.1
pub(crate) const LAYOUT_VERSION: u32 = 3;

/// Ref: `DESIGN.md` §1
pub(crate) const SLOT_SIZE: usize = 0x80;

/// Ref: `DESIGN.md` §1
pub(crate) const SLOT_ALIGN: usize = 8;

/// Ref: `DESIGN.md` §1, §3.4
pub(crate) const ADJ_ARR_SIZE: usize = 0x80;
pub(crate) const ADJ_ARR_SLOTS: usize = 1;
pub(crate) const ADJ_ARRAYS_PER_BLOCK: usize = 6;
pub(crate) const ADJ_ARRAY_ENTRIES: usize = 4;
pub(crate) const ADJ_ENTRY_SIZE: usize = 4;
pub(crate) const ADJ_LIST_SIZE: usize = 0x14;
pub(crate) const ADJ_ARR_OCCUPIED_MASK: u16 = 0x003F;

/// Ref: `DESIGN.md` §1, §3.1
pub(crate) const METADATA_SIZE: usize = 0x80;
pub(crate) const METADATA_SLOTS: usize = 1;
pub(crate) const METADATA_SLOT: u32 = 0;

/// Ref: `DESIGN.md` §1, §3.3
pub(crate) const DBUF_SIZE: usize = 0x0100;
pub(crate) const DBUF_SLOTS: usize = 2;
pub(crate) const DBUF_BUF_COUNT: usize = 31;
pub(crate) const DBUF_BUF_SIZE: usize = 8;
pub(crate) const DBUF_PAYLOAD_SIZE: usize = 248;
pub(crate) const DBUF_FULL_BIT: u32 = 0x8000_0000;
pub(crate) const DBUF_OCCUPANCY_MASK: u32 = 0x7FFF_FFFF;

pub(crate) const DATABUF_SIZE: usize = DBUF_SIZE;
pub(crate) const DATABUF_SLOTS: usize = DBUF_SLOTS;
pub(crate) const DATABUF_BUF_COUNT: usize = DBUF_BUF_COUNT;
pub(crate) const DATABUF_BUF_SIZE: usize = DBUF_BUF_SIZE;
pub(crate) const DATABUF_PAYLOAD_SIZE: usize = DBUF_PAYLOAD_SIZE;
pub(crate) const DATABUF_FULL_BIT: u32 = DBUF_FULL_BIT;
pub(crate) const DATABUF_OCCUPANCY_MASK: u32 = DBUF_OCCUPANCY_MASK;

pub(crate) const CRC_SIZE: usize = 4;
pub(crate) const KV_HEADER_SIZE: usize = 4;
pub(crate) const LIST_KEY_HEADER_SIZE: usize = 5;
pub(crate) const LIST_VAL_HEADER_SIZE: usize = 6;
pub(crate) const LIST_KEY_INLINE_MAX: usize = 3;
pub(crate) const LIST_VAL_INLINE_MAX: usize = 2;

/// Ref: `DESIGN.md` §1, §3.1, §3.2
pub(crate) const NODE_SIZE: usize = 0x80;
pub(crate) const NODE_SLOTS: usize = 1;
pub(crate) const NODE_CHILDREN: usize = 30;
pub(crate) const NODE_FLAG: u32 = 0x0000_0000;
pub(crate) const BUCKET_FLAG: u32 = 0x0000_0001;

pub(crate) const BUCKET_SIZE: usize = 0x80;
pub(crate) const BUCKET_SLOTS: usize = 1;
pub(crate) const BUCKET_ENTRIES: usize = 10;
pub(crate) const HASH_EMPTY: u32 = 0;
pub(crate) const HASH_TOMBSTONE: u32 = 1;
pub(crate) const MIN_VALID_HASH: u32 = 2;
pub(crate) const LIST_FLAG_MASK: u8 = 0x80;
pub(crate) const LIST_FLAG_BE: u32 = 0x8000_0000;
pub(crate) const ADJ_ARR_IDX_MASK: u32 = 0x7FFF_FFFF;
/// Maximum key length in bytes
pub const MAX_KEY_LEN: usize = 0xFF;
/// Maximum value length in bytes
pub const MAX_VAL_LEN: usize = 0xFFFF;

/// Ref: `DESIGN.md` §4.1, §4.2
pub(crate) const HASH_SLICE_BITS: u32 = 5;
pub(crate) const HASH_SLICE_MASK: u32 = 0x1F;
pub(crate) const HASH_SLICES_PER_U32: u32 = 6;
pub(crate) const MAX_TRIE_DEPTH: usize = 0x07;

/// Ref: `DESIGN.md` §4.3
pub(crate) const BUCKET_PROBE_LIMIT: usize = 10;

/// Ref: `DESIGN.md` §1
pub(crate) const PAGE_SIZE_4K: usize = 0x1000;
pub(crate) const PAGE_SIZE_16K: usize = 0x4000;
pub(crate) const PAGE_SIZE_64K: usize = 0x0001_0000;
pub(crate) const PAGE_SIZE: u32 = 0x1000;
pub(crate) const SLOTS_PER_4K_PAGE: usize = 0x20;
pub(crate) const SLOTS_PER_16K_PAGE: usize = 0x80;
pub(crate) const SLOTS_PER_64K_PAGE: usize = 0x0200;

/// Ref: `DESIGN.md` §3.1
pub(crate) const ROOT_NODE_SLOT: u32 = 0x10;
pub(crate) const ROOT_BUCKET_SLOT: u32 = 0x11;
pub(crate) const ADJ_ARR_HEAD_SLOT: u32 = 0x01;
pub(crate) const ADJ_ARR_TAIL_SLOT: u32 = 0x2F;
pub(crate) const DATABUF_HEAD_SLOT: u32 = 0x02;
pub(crate) const INITIAL_BUCKET_START_SLOT: u32 = 0x11;
pub(crate) const INITIAL_BUCKET_COUNT: usize = 30;

/// Ref: `DESIGN.md` §2
pub(crate) const INITIAL_FREE_SLOT_MARKER: u32 = 0x30;
pub(crate) const INITIAL_FOOTPRINT_SLOTS: u32 = 0x30;
pub(crate) const INITIAL_FOOTPRINT_BYTES: usize = 0x1800;
pub(crate) const INITIAL_ADJ_ARR_BLOCKS: usize = 2;
pub(crate) const INITIAL_DATABUF_POOLS: usize = 7;

/// Ref: `DESIGN.md` §8.1
pub(crate) const INITIAL_PAGES_4K: usize = 4;
pub(crate) const INITIAL_PAGES_16K: usize = 1;
pub(crate) const INITIAL_PAGES_64K: usize = 1;

/// Ref: `DESIGN.md` §5.1, §5.3
pub(crate) const ALLOC_SLOT_QUANTUM: u32 = 2;
pub(crate) const ALLOC_BYTE_QUANTUM: usize = 0x0100;

/// Ref: `DESIGN.md` §9.1
pub(crate) const MAX_BUF_IDX: u32 = 0x7FFF_FFFF;
pub(crate) const MAX_DBUF_SLOT: u32 = MAX_BUF_IDX / (DBUF_BUF_COUNT as u32);
pub(crate) const MAX_DATABUFS: u32 = MAX_DBUF_SLOT / (DBUF_SLOTS as u32);
pub(crate) const MAX_STORAGE_PAGES: u32 = MAX_DBUF_SLOT.div_ceil(SLOTS_PER_4K_PAGE as u32);
pub(crate) const MAX_SLOTS: u64 = 1 << 0x20;
pub(crate) const MAX_STORAGE_PAGES_4K: u32 = 0x0800_0000;
pub(crate) const MAX_DB_LEN: usize = 0x80_0000_0000;

pub(crate) const OCC_MAX_RETRIES: u32 = 0x100;

/// Ref: `DESIGN.md` §3.1, §3.2
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SlotHeader {
    pub(crate) flag: u32,
    pub(crate) padding: [u8; 4],
}

const _: () = assert!(core::mem::size_of::<SlotHeader>() == 8);
const _: () = assert!(core::mem::align_of::<SlotHeader>() == 8);

/// Ref: `DESIGN.md` §1, §3
#[repr(C, align(8))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Slot(pub(crate) [u8; SLOT_SIZE]);

const _: () = assert!(core::mem::size_of::<Slot>() == SLOT_SIZE);
const _: () = assert!(core::mem::align_of::<Slot>() == SLOT_ALIGN);

impl Default for Slot {
    #[inline(always)]
    fn default() -> Self {
        Self::zeroed()
    }
}

impl Deref for Slot {
    type Target = [u8; SLOT_SIZE];

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Slot {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl AsRef<[u8]> for Slot {
    #[inline(always)]
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl AsMut<[u8]> for Slot {
    #[inline(always)]
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

impl Slot {
    pub(crate) const SIZE: usize = SLOT_SIZE;
    pub(crate) const ALIGN: usize = SLOT_ALIGN;

    #[inline(always)]
    pub(crate) const fn new(raw: [u8; SLOT_SIZE]) -> Self {
        Self(raw)
    }

    #[inline(always)]
    pub(crate) const fn zeroed() -> Self {
        Self([0u8; SLOT_SIZE])
    }

    #[inline(always)]
    pub(crate) const fn as_slice(&self) -> &[u8] {
        &self.0
    }

    #[inline(always)]
    pub(crate) fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.0
    }

    #[inline(always)]
    pub(crate) fn flag(&self) -> u32 {
        u32::from_le_bytes([self.0[0], self.0[1], self.0[2], self.0[3]])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_slot_layout_invariants() {
        assert_eq!(core::mem::size_of::<Slot>(), 0x80);
        assert_eq!(core::mem::align_of::<Slot>(), 8);
        assert_eq!(Slot::SIZE, 0x80);
        assert_eq!(Slot::ALIGN, 8);
    }

    #[test]
    fn ok_slot_zeroed_and_default() {
        let s = Slot::zeroed();
        assert_eq!(s.0, [0u8; 0x80]);
        assert_eq!(Slot::default(), s);
        assert_eq!(s.as_slice(), &[0u8; 0x80]);
    }

    #[test]
    fn ok_slot_deref_mut() {
        let mut s = Slot::zeroed();
        s[0] = 0xAA;
        s[0x7F] = 0xBB;
        assert_eq!(s.0[0], 0xAA);
        assert_eq!(s.0[0x7F], 0xBB);
        assert_eq!(s.as_mut_slice()[0], 0xAA);
    }

    #[test]
    fn ok_magic_and_seed() {
        assert_eq!(MAGIC, *b"TBFX");
        assert_eq!(SEED, 0x5442_4658);
        assert_eq!(SEED, u32::from_be_bytes(MAGIC));
    }

    #[test]
    fn ok_static_head_constants() {
        assert_eq!(ROOT_NODE_SLOT, 0x10);
        assert_eq!(ADJ_ARR_HEAD_SLOT, 0x01);
        assert_eq!(DATABUF_HEAD_SLOT, 0x02);
        assert_eq!(INITIAL_FREE_SLOT_MARKER, 0x30);
    }

    #[test]
    fn ok_capacity_proof_invariants() {
        assert_eq!(MAX_DBUF_SLOT, 69273666);
        assert_eq!(MAX_DATABUFS, 34636833);
        assert_eq!(MAX_DB_LEN, 0x80_0000_0000);
        assert_eq!(MAX_DB_LEN, 512 * 1024 * 1024 * 1024);
        assert_eq!(MAX_SLOTS, 1u64 << 0x20);
        assert_eq!((MAX_SLOTS as usize) * SLOT_SIZE, MAX_DB_LEN);
    }
}
