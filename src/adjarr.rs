use crate::common::{
    ADJ_ARR_OCCUPIED_MASK, ADJ_ARR_SIZE, ADJ_ARR_SLOTS, ADJ_ARRAY_ENTRIES, ADJ_ARRAYS_PER_BLOCK,
    ADJ_ENTRY_SIZE, ADJ_LIST_SIZE, Slot,
};

/// Ref: `DESIGN.md` §3.4
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) struct AdjArrayEntry {
    dbuf_idx: u32,
}

const _: () = assert!(ADJ_ENTRY_SIZE == 4);
const _: () = assert!(core::mem::align_of::<AdjArrayEntry>() == 4);
const _: () = assert!(core::mem::offset_of!(AdjArrayEntry, dbuf_idx) == 0);
const _: () = assert!(core::mem::size_of::<AdjArrayEntry>() == ADJ_ENTRY_SIZE);

impl AdjArrayEntry {
    #[inline(always)]
    pub(crate) const fn empty() -> Self {
        Self { dbuf_idx: 0 }
    }

    #[inline(always)]
    #[allow(dead_code)]
    pub(crate) const fn new(dbuf_idx: u32) -> Self {
        Self { dbuf_idx }
    }

    #[inline(always)]
    pub(crate) fn is_empty(&self) -> bool {
        self.dbuf_idx == 0
    }

    #[inline(always)]
    #[allow(dead_code)]
    pub(crate) fn is_occupied(&self) -> bool {
        self.dbuf_idx != 0
    }

    #[inline(always)]
    pub(crate) fn buf_idx(&self) -> u32 {
        self.dbuf_idx
    }

    #[inline(always)]
    pub(crate) fn set_buf_idx(&mut self, idx: u32) {
        self.dbuf_idx = idx;
    }

    #[inline(always)]
    pub(crate) fn clear(&mut self) {
        self.dbuf_idx = 0;
    }
}

/// Ref: `DESIGN.md` §3.4
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AdjArrayList {
    pub(crate) entries: [AdjArrayEntry; ADJ_ARRAY_ENTRIES],
    pub(crate) next_arr_idx: u32,
}

const _: () = assert!(core::mem::align_of::<AdjArrayList>() == 4);
const _: () = assert!(core::mem::offset_of!(AdjArrayList, entries) == 0);
const _: () = assert!(core::mem::size_of::<AdjArrayList>() == ADJ_LIST_SIZE);
const _: () = assert!(core::mem::offset_of!(AdjArrayList, next_arr_idx) == 0x10);

const _: () = assert!(ADJ_LIST_SIZE == 0x14);
const _: () = assert!(ADJ_ARRAY_ENTRIES * ADJ_ENTRY_SIZE + 4 == ADJ_LIST_SIZE);
const _: () = assert!(core::mem::size_of::<[AdjArrayEntry; ADJ_ARRAY_ENTRIES]>() == 0x10);

impl Default for AdjArrayList {
    #[inline(always)]
    fn default() -> Self {
        Self::new()
    }
}

impl AdjArrayList {
    pub(crate) const ENTRY_COUNT: usize = ADJ_ARRAY_ENTRIES;

    #[inline(always)]
    pub(crate) const fn new() -> Self {
        Self { entries: [AdjArrayEntry::empty(); ADJ_ARRAY_ENTRIES], next_arr_idx: 0 }
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
    pub(crate) fn push(&mut self, buf_idx: u32) -> Option<usize> {
        for (i, entry) in self.entries.iter_mut().enumerate() {
            if entry.is_empty() {
                entry.set_buf_idx(buf_idx);
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
    #[allow(dead_code)]
    pub(crate) fn entry_mut(&mut self, idx: usize) -> Option<&mut AdjArrayEntry> {
        if idx < Self::ENTRY_COUNT { Some(&mut self.entries[idx]) } else { None }
    }

    #[inline(always)]
    pub(crate) const fn next_array(&self) -> u32 {
        self.next_arr_idx
    }

    #[inline(always)]
    pub(crate) fn set_next_array(&mut self, next: u32) {
        self.next_arr_idx = next;
    }

    #[inline(always)]
    pub(crate) fn clear(&mut self) {
        *self = Self::new();
    }
}

/// Ref: `DESIGN.md` §3.4
#[inline(always)]
pub(crate) const fn relative_array_idx(block_hop: u32, local_array: usize) -> u32 {
    block_hop * (ADJ_ARRAYS_PER_BLOCK as u32) + (local_array as u32)
}

/// Ref: `DESIGN.md` §3.4
#[inline(always)]
pub(crate) const fn decompose_relative_array_idx(relative_idx: u32) -> (u32, usize) {
    let block_hop = relative_idx / (ADJ_ARRAYS_PER_BLOCK as u32);
    let local_array = (relative_idx % (ADJ_ARRAYS_PER_BLOCK as u32)) as usize;

    (block_hop, local_array)
}

/// Ref: `DESIGN.md` §1, §3.4
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AdjArrayBlock {
    pub(crate) arrays: [AdjArrayList; ADJ_ARRAYS_PER_BLOCK],
    pub(crate) next_adjarr_idx: u32,
    pub(crate) bmap: u16,
    pub(crate) padding: [u8; 2],
}

const _: () = assert!(core::mem::align_of::<AdjArrayBlock>() == 8);
const _: () = assert!(core::mem::offset_of!(AdjArrayBlock, arrays) == 0);
const _: () = assert!(core::mem::offset_of!(AdjArrayBlock, bmap) == 0x7C);
const _: () = assert!(core::mem::offset_of!(AdjArrayBlock, padding) == 0x7E);
const _: () = assert!(core::mem::size_of::<AdjArrayBlock>() == ADJ_ARR_SIZE);
const _: () = assert!(core::mem::offset_of!(AdjArrayBlock, next_adjarr_idx) == 0x78);
const _: () = assert!(core::mem::size_of::<AdjArrayBlock>() == ADJ_ARR_SLOTS * Slot::SIZE);
const _: () = assert!(core::mem::size_of::<[AdjArrayList; ADJ_ARRAYS_PER_BLOCK]>() == 0x78);

const _: () = assert!(ADJ_ARR_SIZE == ADJ_ARR_SLOTS * Slot::SIZE);
const _: () = assert!(ADJ_ARR_SIZE % Slot::SIZE == 0);
const _: () = assert!(ADJ_ARR_OCCUPIED_MASK == 0x003F);
const _: () = assert!(ADJ_ARRAYS_PER_BLOCK * ADJ_LIST_SIZE == 0x78);
const _: () = assert!((1u16 << ADJ_ARRAYS_PER_BLOCK) - 1 == ADJ_ARR_OCCUPIED_MASK);

const _: () = assert!(relative_array_idx(0, 0) == 0);
const _: () = assert!(relative_array_idx(2, 3) == 0x0F);
const _: () = assert!(decompose_relative_array_idx(0).0 == 0);
const _: () = assert!(decompose_relative_array_idx(0).1 == 0);
const _: () = assert!(decompose_relative_array_idx(0x0F).0 == 2);
const _: () = assert!(decompose_relative_array_idx(0x0F).1 == 3);

impl Default for AdjArrayBlock {
    #[inline(always)]
    fn default() -> Self {
        Self::new()
    }
}

impl AdjArrayBlock {
    pub(crate) const ARRAY_COUNT: usize = ADJ_ARRAYS_PER_BLOCK;

    #[inline(always)]
    pub(crate) const fn new() -> Self {
        Self {
            arrays: [AdjArrayList::new(); ADJ_ARRAYS_PER_BLOCK],
            next_adjarr_idx: 0,
            bmap: 0,
            padding: [0u8; 2],
        }
    }

    #[inline(always)]
    pub(crate) fn next_block(&self) -> u32 {
        self.next_adjarr_idx
    }

    #[inline(always)]
    pub(crate) fn set_next_block(&mut self, next: u32) {
        self.next_adjarr_idx = next;
    }

    #[inline(always)]
    #[allow(dead_code)]
    pub(crate) fn is_full(&self) -> bool {
        (self.bmap & ADJ_ARR_OCCUPIED_MASK) == ADJ_ARR_OCCUPIED_MASK
    }

    #[inline(always)]
    pub(crate) fn is_array_occupied(&self, idx: usize) -> bool {
        // sanity check
        debug_assert!(idx < Self::ARRAY_COUNT);

        (self.bmap & (1 << idx)) != 0
    }

    #[inline(always)]
    pub(crate) fn set_array_occupied(&mut self, idx: usize, occupied: bool) {
        // sanity check
        debug_assert!(idx < Self::ARRAY_COUNT);

        if occupied {
            return self.bmap |= 1 << idx;
        }

        self.bmap &= !(1 << idx);
    }

    pub(crate) fn alloc_array(&mut self) -> Option<usize> {
        for i in 0..Self::ARRAY_COUNT {
            if !self.is_array_occupied(i) {
                self.set_array_occupied(i, true);
                return Some(i);
            }
        }

        None
    }

    pub(crate) fn free_array(&mut self, idx: usize) {
        // sanity check
        debug_assert!(idx < Self::ARRAY_COUNT);

        self.set_array_occupied(idx, false);
        self.arrays[idx].clear();
    }

    #[inline(always)]
    pub(crate) fn array(&self, idx: usize) -> &AdjArrayList {
        // sanity check
        debug_assert!(idx < Self::ARRAY_COUNT);

        &self.arrays[idx]
    }

    #[inline(always)]
    pub(crate) fn array_mut(&mut self, idx: usize) -> &mut AdjArrayList {
        // sanity check
        debug_assert!(idx < Self::ARRAY_COUNT);

        &mut self.arrays[idx]
    }

    #[inline(always)]
    #[allow(dead_code)]
    pub(crate) fn as_slot(&self) -> &Slot {
        unsafe { &*core::ptr::from_ref(self).cast() }
    }

    #[inline(always)]
    #[allow(dead_code)]
    pub(crate) fn as_slot_mut(&mut self) -> &mut Slot {
        unsafe { &mut *core::ptr::from_mut(self).cast() }
    }

    #[inline(always)]
    pub(crate) fn as_slots(&self) -> &[Slot; ADJ_ARR_SLOTS] {
        unsafe { &*core::ptr::from_ref(self).cast() }
    }

    #[inline(always)]
    #[allow(dead_code)]
    pub(crate) fn as_slots_mut(&mut self) -> &mut [Slot; ADJ_ARR_SLOTS] {
        unsafe { &mut *core::ptr::from_mut(self).cast() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_adjarr_layout_invariants() {
        assert_eq!(core::mem::size_of::<AdjArrayEntry>(), 4);
        assert_eq!(core::mem::align_of::<AdjArrayEntry>(), 4);
        assert_eq!(core::mem::size_of::<AdjArrayList>(), 0x14);
        assert_eq!(core::mem::align_of::<AdjArrayList>(), 4);
        assert_eq!(core::mem::size_of::<AdjArrayBlock>(), 0x80);
        assert_eq!(core::mem::align_of::<AdjArrayBlock>(), 8);
        assert_eq!(core::mem::offset_of!(AdjArrayBlock, arrays), 0);
        assert_eq!(core::mem::offset_of!(AdjArrayBlock, next_adjarr_idx), 0x78);
        assert_eq!(core::mem::offset_of!(AdjArrayBlock, bmap), 0x7C);
        assert_eq!(core::mem::offset_of!(AdjArrayBlock, padding), 0x7E);
    }

    #[test]
    fn ok_adjarr_entry_operations() {
        let mut entry = AdjArrayEntry::empty();
        assert!(entry.is_empty());
        assert!(!entry.is_occupied());
        assert_eq!(entry.buf_idx(), 0);

        entry.set_buf_idx(0x0123_4567);
        assert!(!entry.is_empty());
        assert!(entry.is_occupied());
        assert_eq!(entry.buf_idx(), 0x0123_4567);

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

        assert_eq!(list.push(0x0100), Some(0));
        assert_eq!(list.len(), 1);
        assert!(!list.is_empty());
        assert!(!list.is_full());

        assert_eq!(list.push(0x0200), Some(1));
        assert_eq!(list.push(0x0300), Some(2));
        assert_eq!(list.push(0x0400), Some(3));
        assert_eq!(list.len(), 4);
        assert!(list.is_full());

        assert_eq!(list.push(0x0500), None);

        assert_eq!(list.entry(0).unwrap().buf_idx(), 0x0100);
        assert_eq!(list.entry(1).unwrap().buf_idx(), 0x0200);
        assert_eq!(list.entry(2).unwrap().buf_idx(), 0x0300);
        assert_eq!(list.entry(3).unwrap().buf_idx(), 0x0400);
        assert_eq!(list.entry(4), None);

        list.set_next_array(0x000F);
        assert_eq!(list.next_array(), 0x000F);

        let popped4 = list.pop().unwrap();
        assert_eq!(popped4.buf_idx(), 0x0400);
        assert_eq!(list.len(), 3);

        let popped3 = list.pop().unwrap();
        assert_eq!(popped3.buf_idx(), 0x0300);
        let popped2 = list.pop().unwrap();
        assert_eq!(popped2.buf_idx(), 0x0200);
        let popped1 = list.pop().unwrap();
        assert_eq!(popped1.buf_idx(), 0x0100);
        assert_eq!(list.pop(), None);
        assert!(list.is_empty());
    }

    #[test]
    fn ok_adjarr_relative_indexing() {
        assert_eq!(relative_array_idx(0, 0), 0);
        assert_eq!(decompose_relative_array_idx(0), (0, 0));

        assert_eq!(relative_array_idx(0, 5), 5);
        assert_eq!(decompose_relative_array_idx(5), (0, 5));

        assert_eq!(relative_array_idx(2, 3), 0x0F);
        assert_eq!(decompose_relative_array_idx(0x0F), (2, 3));
    }

    #[test]
    fn ok_adjarr_block_alloc_and_free() {
        let mut block = AdjArrayBlock::new();
        assert_eq!(block.next_block(), 0);
        assert!(!block.is_full());

        block.set_next_block(0x0012_3456);
        assert_eq!(block.next_block(), 0x0012_3456);

        for i in 0..AdjArrayBlock::ARRAY_COUNT {
            assert_eq!(block.alloc_array(), Some(i));
        }
        assert!(block.is_full());
        assert_eq!(block.alloc_array(), None);

        let _ = block.array_mut(2).push(0x000A);
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
        block.set_next_block(0x0007);
        block.set_array_occupied(0, true);
        let _ = block.array_mut(0).push(0x0ABC_DEF0);

        let slot = block.as_slot();
        assert_eq!(slot.0.len(), 0x40);
        assert_eq!(block.as_slots().len(), 2);
    }
}
