//! Adjacent Array block for 1-to-N list structures (0x80 Bytes / 1 Slot)
//!
//! Ref: `DESIGN` §3, §5, §7

use crate::slot::Slot;

#[repr(C, packed(2))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) struct AdjArrayEntry {
    buf_idx: u32,
    vlen: u16,
}

const _: () = assert!(core::mem::size_of::<AdjArrayEntry>() == 6);
const _: () = assert!(core::mem::align_of::<AdjArrayEntry>() == 2);

impl AdjArrayEntry {
    #[inline(always)]
    pub(crate) const fn empty() -> Self {
        Self { buf_idx: 0, vlen: 0 }
    }

    #[inline(always)]
    pub(crate) fn is_empty(&self) -> bool {
        self.buf_idx == 0 && self.vlen == 0
    }

    #[inline(always)]
    pub(crate) fn buf_idx(&self) -> u32 {
        self.buf_idx
    }

    #[inline(always)]
    pub(crate) fn vlen(&self) -> u16 {
        self.vlen
    }

    #[inline(always)]
    pub(crate) fn set(&mut self, buf_idx: u32, vlen: u16) {
        self.buf_idx = buf_idx;
        self.vlen = vlen;
    }

    #[inline(always)]
    pub(crate) fn clear(&mut self) {
        *self = Self::empty();
    }
}

#[repr(C, packed(2))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) struct AdjArrayList {
    pub(crate) entries: [AdjArrayEntry; 3],
    pub(crate) next_arr: u16,
}

const _: () = assert!(core::mem::size_of::<AdjArrayList>() == 0x14);
const _: () = assert!(core::mem::align_of::<AdjArrayList>() == 2);

impl AdjArrayList {
    pub(crate) const ENTRY_COUNT: usize = 3;

    #[inline(always)]
    pub(crate) const fn new() -> Self {
        Self { entries: [AdjArrayEntry::empty(); 3], next_arr: 0 }
    }

    #[inline(always)]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.iter().all(|e| e.is_empty())
    }

    #[inline(always)]
    pub(crate) fn is_full(&self) -> bool {
        self.entries.iter().all(|e| !e.is_empty())
    }

    #[inline(always)]
    pub(crate) fn len(&self) -> usize {
        self.entries.iter().filter(|e| !e.is_empty()).count()
    }

    #[inline(always)]
    pub(crate) fn push(&mut self, buf_idx: u32, vlen: u16) -> Option<usize> {
        for (i, entry) in self.entries.iter_mut().enumerate() {
            if entry.is_empty() {
                entry.set(buf_idx, vlen);
                return Some(i);
            }
        }
        None
    }

    #[inline(always)]
    pub(crate) fn pop(&mut self) -> Option<AdjArrayEntry> {
        for entry in self.entries.iter_mut().rev() {
            if !entry.is_empty() {
                let val = *entry;
                entry.clear();
                return Some(val);
            }
        }
        None
    }

    #[inline(always)]
    pub(crate) fn entry(&self, idx: usize) -> Option<&AdjArrayEntry> {
        if idx < Self::ENTRY_COUNT { Some(&self.entries[idx]) } else { None }
    }

    #[inline(always)]
    pub(crate) const fn next_array(&self) -> u16 {
        self.next_arr
    }

    #[inline(always)]
    pub(crate) fn set_next_array(&mut self, next: u16) {
        self.next_arr = next;
    }

    #[inline(always)]
    pub(crate) fn clear(&mut self) {
        *self = Self::new();
    }
}

#[inline(always)]
pub(crate) const fn global_index(block: u32, array: usize) -> u16 {
    (block as u16) * (AdjArrayBlock::ARRAY_COUNT as u16) + (array as u16)
}

#[inline(always)]
pub(crate) const fn decompose_global_index(global_idx: u16) -> (u32, usize) {
    let block = (global_idx / (AdjArrayBlock::ARRAY_COUNT as u16)) as u32;
    let array = (global_idx % (AdjArrayBlock::ARRAY_COUNT as u16)) as usize;
    (block, array)
}

#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AdjArrayBlock {
    header: u32,
    arrays: [AdjArrayList; 6],
    bitmap: u16,
    padding: [u8; 2],
}

const _: () = assert!(core::mem::size_of::<AdjArrayBlock>() == 0x80);
const _: () = assert!(core::mem::align_of::<AdjArrayBlock>() == 8);

impl Default for AdjArrayBlock {
    #[inline(always)]
    fn default() -> Self {
        Self::new()
    }
}

impl AdjArrayBlock {
    pub(crate) const ARRAY_COUNT: usize = 6;
    pub(crate) const OCCUPIED_BIT: u32 = 0x8000_0000;
    pub(crate) const INDEX_MASK: u32 = 0x7FFF_FFFF;
    pub(crate) const BITMAP_MASK: u16 = 0x003F;

    #[inline(always)]
    pub(crate) const fn new() -> Self {
        Self { header: 0, arrays: [AdjArrayList::new(); 6], bitmap: 0, padding: [0u8; 2] }
    }

    #[inline(always)]
    pub(crate) fn set_occupied(&mut self, occupied: bool) {
        if occupied {
            self.header |= Self::OCCUPIED_BIT;
        } else {
            self.header &= !Self::OCCUPIED_BIT;
        }
    }

    #[inline(always)]
    pub(crate) fn next_block(&self) -> u32 {
        self.header & Self::INDEX_MASK
    }

    #[inline(always)]
    pub(crate) fn set_next_block(&mut self, next: u32) {
        let occ = self.header & Self::OCCUPIED_BIT;
        self.header = occ | (next & Self::INDEX_MASK);
    }

    #[inline(always)]
    pub(crate) fn is_array_occupied(&self, idx: usize) -> bool {
        debug_assert!(idx < Self::ARRAY_COUNT);
        (self.bitmap & (1u16 << idx)) != 0
    }

    #[inline(always)]
    pub(crate) fn set_array_occupied(&mut self, idx: usize, occupied: bool) {
        debug_assert!(idx < Self::ARRAY_COUNT);
        if occupied {
            self.bitmap |= 1u16 << idx;
        } else {
            self.bitmap &= !(1u16 << idx);
        }
    }

    #[inline(always)]
    pub(crate) fn alloc_array(&mut self) -> Option<usize> {
        for idx in 0..Self::ARRAY_COUNT {
            if !self.is_array_occupied(idx) {
                self.set_array_occupied(idx, true);
                return Some(idx);
            }
        }
        None
    }

    #[inline(always)]
    pub(crate) fn free_array(&mut self, idx: usize) {
        debug_assert!(idx < Self::ARRAY_COUNT);
        self.set_array_occupied(idx, false);
        self.arrays[idx].clear();
    }

    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn occupied_count(&self) -> usize {
        (self.bitmap & Self::BITMAP_MASK).count_ones() as usize
    }

    #[inline(always)]
    pub(crate) fn is_full(&self) -> bool {
        (self.bitmap & Self::BITMAP_MASK) == Self::BITMAP_MASK
    }

    #[inline(always)]
    pub(crate) fn array(&self, idx: usize) -> &AdjArrayList {
        debug_assert!(idx < Self::ARRAY_COUNT);
        &self.arrays[idx]
    }

    #[inline(always)]
    pub(crate) fn array_mut(&mut self, idx: usize) -> &mut AdjArrayList {
        debug_assert!(idx < Self::ARRAY_COUNT);
        &mut self.arrays[idx]
    }

    #[inline(always)]
    pub(crate) fn as_slot(&self) -> &Slot {
        unsafe { &*(self as *const Self as *const Slot) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_adjarr_layout_invariants() {
        assert_eq!(core::mem::size_of::<AdjArrayEntry>(), 6);
        assert_eq!(core::mem::align_of::<AdjArrayEntry>(), 2);
        assert_eq!(core::mem::size_of::<AdjArrayList>(), 0x14);
        assert_eq!(core::mem::align_of::<AdjArrayList>(), 2);
        assert_eq!(core::mem::size_of::<AdjArrayBlock>(), 0x80);
        assert_eq!(core::mem::align_of::<AdjArrayBlock>(), 8);
        assert_eq!(core::mem::size_of::<Slot>(), 0x80);
    }

    #[test]
    fn ok_adjarr_field_offsets() {
        assert_eq!(core::mem::offset_of!(AdjArrayBlock, header), 0);
        assert_eq!(core::mem::offset_of!(AdjArrayBlock, arrays), 4);
        assert_eq!(core::mem::offset_of!(AdjArrayBlock, bitmap), 0x7C);
        assert_eq!(core::mem::offset_of!(AdjArrayBlock, padding), 0x7E);
    }

    #[test]
    fn ok_adjarr_entry_operations() {
        let mut entry = AdjArrayEntry::empty();
        assert!(entry.is_empty());
        assert_eq!(entry.buf_idx(), 0);
        assert_eq!(entry.vlen(), 0);

        entry.set(0x0123_4567, 0x002A);
        assert!(!entry.is_empty());
        assert_eq!(entry.buf_idx(), 0x0123_4567);
        assert_eq!(entry.vlen(), 0x002A);

        entry.clear();
        assert!(entry.is_empty());
        assert_eq!(entry, AdjArrayEntry::empty());
    }

    #[test]
    fn ok_adjarr_list_push_pop_and_indexing() {
        let mut list = AdjArrayList::new();
        assert!(list.is_empty());
        assert!(!list.is_full());
        assert_eq!(list.len(), 0);
        assert_eq!(list.next_array(), 0);

        assert_eq!(list.push(0x0100, 0x10), Some(0));
        assert_eq!(list.len(), 1);
        assert!(!list.is_empty());
        assert!(!list.is_full());

        assert_eq!(list.push(0x0200, 0x20), Some(1));
        assert_eq!(list.push(0x0300, 0x30), Some(2));
        assert_eq!(list.len(), 3);
        assert!(list.is_full());

        assert_eq!(list.push(0x0400, 0x40), None);

        assert_eq!(list.entry(0).unwrap().buf_idx(), 0x0100);
        assert_eq!(list.entry(1).unwrap().buf_idx(), 0x0200);
        assert_eq!(list.entry(2).unwrap().buf_idx(), 0x0300);
        assert_eq!(list.entry(3), None);

        list.set_next_array(0x000F);
        assert_eq!(list.next_array(), 0x000F);

        let popped3 = list.pop().unwrap();
        assert_eq!(popped3.buf_idx(), 0x0300);
        assert_eq!(popped3.vlen(), 0x30);
        assert_eq!(list.len(), 2);
        let popped2 = list.pop().unwrap();
        assert_eq!(popped2.buf_idx(), 0x0200);
        assert_eq!(popped2.vlen(), 0x20);
        let popped1 = list.pop().unwrap();
        assert_eq!(popped1.buf_idx(), 0x0100);
        assert_eq!(popped1.vlen(), 0x10);
        assert_eq!(list.pop(), None);
        assert!(list.is_empty());
    }

    #[test]
    fn ok_adjarr_global_indexing() {
        assert_eq!(global_index(0, 0), 0);
        assert_eq!(decompose_global_index(0), (0, 0));

        assert_eq!(global_index(0, 5), 5);
        assert_eq!(decompose_global_index(5), (0, 5));

        assert_eq!(global_index(2, 3), 0x0F);
        assert_eq!(decompose_global_index(0x0F), (2, 3));
    }

    #[test]
    fn ok_adjarr_block_alloc_and_freelist() {
        let mut block = AdjArrayBlock::new();
        assert_eq!(block.next_block(), 0);
        assert!(!block.is_full());

        block.set_occupied(true);
        block.set_next_block(0x0012_3456);
        assert_eq!(block.next_block(), 0x0012_3456);

        for i in 0..AdjArrayBlock::ARRAY_COUNT {
            assert_eq!(block.alloc_array(), Some(i));
        }
        assert!(block.is_full());
        assert_eq!(block.alloc_array(), None);

        block.array_mut(2).push(0x000A, 0x000B);
        assert_eq!(block.array(2).len(), 1);
        block.free_array(2);
        assert!(!block.is_full());
        assert!(!block.is_array_occupied(2));
        assert_eq!(block.array(2).len(), 0);

        assert_eq!(block.alloc_array(), Some(2));
        assert!(block.is_full());
    }

    #[test]
    fn ok_adjarr_as_slot() {
        let mut block = AdjArrayBlock::new();
        block.set_occupied(true);
        block.set_next_block(0x0007);
        block.set_array_occupied(0, true);
        block.array_mut(0).push(0x0ABC_DEF0, 0x0012);

        let slot = block.as_slot();
        assert_eq!(slot.0.len(), 0x80);
    }
}
