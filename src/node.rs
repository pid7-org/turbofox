//! Trie branch Node structure (0x0100 Bytes / 2 Slots)
//!
//! Ref: `DESIGN.md` §3.2, §3.3, §4.2, §5.2, §7.1

use crate::slot::Slot;

#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Node {
    pub(crate) children: [u32; 0x40],
}

const _: () = assert!(core::mem::size_of::<Node>() == 0x0100);
const _: () = assert!(core::mem::align_of::<Node>() == 8);

impl Default for Node {
    #[inline(always)]
    fn default() -> Self {
        Self { children: [0u32; 0x40] }
    }
}

impl Node {
    pub(crate) const SIZE: usize = 0x0100;
    pub(crate) const CHILD_COUNT: usize = 0x40;
    pub(crate) const MASK_INDEX: u32 = 0x7FFF_FFFF;
    pub(crate) const DISCRIMINATOR_BIT: u32 = 0x8000_0000;
    pub(crate) const BITS_PER_LEVEL: u32 = 6;
    pub(crate) const CHILD_INDEX_MASK: u32 = 0x3F;
    pub(crate) const HASH_BITS: u32 = 0x20;

    #[inline(always)]
    pub(crate) const fn empty() -> Self {
        Self { children: [0u32; 0x40] }
    }

    /// Computes cyclic 6-bit child branch index for hash at given depth
    #[inline(always)]
    pub(crate) const fn child_index(hash: u32, depth: u32) -> usize {
        let shift = (depth * Self::BITS_PER_LEVEL) % Self::HASH_BITS;
        ((hash >> shift) & Self::CHILD_INDEX_MASK) as usize
    }

    /// Retrieves child virtual page index at `idx` masked to bits 0..0x1E
    #[inline(always)]
    pub(crate) fn get_child(&self, idx: usize) -> u32 {
        self.children[idx] & Self::MASK_INDEX
    }

    /// Sets child virtual page index at `idx`, ensuring bit 0x1F remains 0
    #[inline(always)]
    pub(crate) fn set_child(&mut self, idx: usize, child_idx: u32) {
        self.children[idx] = child_idx & Self::MASK_INDEX;
    }

    /// Routes key hash at given depth and returns masked child virtual page index
    #[inline(always)]
    pub(crate) fn route_child(&self, hash: u32, depth: u32) -> u32 {
        self.get_child(Self::child_index(hash, depth))
    }

    /// Verifies that this page's discriminator bit indicates a Node (`header & 0x8000_0000 == 0`)
    #[inline(always)]
    pub(crate) fn is_node(&self) -> bool {
        (self.children[0] & Self::DISCRIMINATOR_BIT) == 0
    }

    /// Checks if child entry at `idx` is unlinked (equals 0)
    #[inline(always)]
    pub(crate) fn is_child_empty(&self, idx: usize) -> bool {
        self.get_child(idx) == 0
    }

    /// Returns Some(vp) if child entry is allocated, or None if unlinked
    #[inline(always)]
    pub(crate) fn child_vp(&self, idx: usize) -> Option<u32> {
        let vp = self.get_child(idx);
        if vp == 0 { None } else { Some(vp) }
    }

    /// Clears child entry at `idx` to 0
    #[inline(always)]
    pub(crate) fn clear_child(&mut self, idx: usize) {
        self.children[idx] = 0;
    }

    /// Counts non-zero child entries active in this branch
    #[inline(always)]
    pub(crate) fn occupied_count(&self) -> usize {
        self.children.iter().filter(|&&c| (c & Self::MASK_INDEX) != 0).count()
    }

    /// Returns true if all 0x40 child entries are 0
    #[inline(always)]
    pub(crate) fn is_empty(&self) -> bool {
        self.children.iter().all(|&c| (c & Self::MASK_INDEX) == 0)
    }

    /// Returns true if all 0x40 child entries are non-zero
    #[inline(always)]
    pub(crate) fn is_full(&self) -> bool {
        self.children.iter().all(|&c| (c & Self::MASK_INDEX) != 0)
    }

    /// Iterates over populated (child_idx, vp) entries
    #[inline(always)]
    pub(crate) fn iter_children(&self) -> impl Iterator<Item = (usize, u32)> + '_ {
        self.children
            .iter()
            .enumerate()
            .map(|(idx, &c)| (idx, c & Self::MASK_INDEX))
            .filter(|&(_, vp)| vp != 0)
    }

    /// Constructs a Node mapping all 0x40 children to contiguous virtual pages
    #[inline(always)]
    pub(crate) fn init_from_contiguous(base_vp: u32) -> Self {
        let mut node = Self::empty();
        node.populate_range(base_vp);
        node
    }

    /// Fills all 0x40 entries with sequential target child VPs starting at base_vp
    #[inline(always)]
    pub(crate) fn populate_range(&mut self, base_vp: u32) {
        for i in 0..Self::CHILD_COUNT {
            self.set_child(i, (base_vp + (i as u32)) & Self::MASK_INDEX);
        }
    }

    /// Verifies discriminator and asserts bit 0x1F == 0 across all 0x40 child entries
    #[inline(always)]
    pub(crate) fn validate_invariants(&self) -> bool {
        self.children.iter().all(|&c| (c & Self::DISCRIMINATOR_BIT) == 0)
    }

    /// Zero-copy byte slice view
    #[inline(always)]
    pub(crate) fn as_bytes(&self) -> &[u8; Self::SIZE] {
        unsafe { &*(self as *const Self as *const [u8; Self::SIZE]) }
    }

    /// Zero-copy mutable byte slice view
    #[inline(always)]
    pub(crate) fn as_bytes_mut(&mut self) -> &mut [u8; Self::SIZE] {
        unsafe { &mut *(self as *mut Self as *mut [u8; Self::SIZE]) }
    }

    /// Zero-copy reference from raw 0x0100 byte buffer
    #[inline(always)]
    pub(crate) fn from_bytes(bytes: &[u8; Self::SIZE]) -> &Self {
        debug_assert_eq!(bytes.as_ptr().align_offset(8), 0);
        unsafe { &*(bytes.as_ptr() as *const Self) }
    }

    /// Zero-copy mutable reference from raw 0x0100 byte buffer
    #[inline(always)]
    pub(crate) fn from_bytes_mut(bytes: &mut [u8; Self::SIZE]) -> &mut Self {
        debug_assert_eq!(bytes.as_mut_ptr().align_offset(8), 0);
        unsafe { &mut *(bytes.as_mut_ptr() as *mut Self) }
    }

    /// Converts a pair of 0x80-byte Slots into a Node reference
    #[inline(always)]
    pub(crate) fn from_slots(slots: &[Slot; 2]) -> &Self {
        unsafe { &*(slots.as_ptr() as *const Self) }
    }

    /// Converts a mutable pair of 0x80-byte Slots into a mutable Node reference
    #[inline(always)]
    pub(crate) fn from_slots_mut(slots: &mut [Slot; 2]) -> &mut Self {
        unsafe { &mut *(slots.as_mut_ptr() as *mut Self) }
    }

    /// Views Node as a pair of 0x80-byte Slots
    #[inline(always)]
    pub(crate) fn as_slots(&self) -> &[Slot; 2] {
        unsafe { &*(self as *const Self as *const [Slot; 2]) }
    }

    /// Views Node as a mutable pair of 0x80-byte Slots
    #[inline(always)]
    pub(crate) fn as_slots_mut(&mut self) -> &mut [Slot; 2] {
        unsafe { &mut *(self as *mut Self as *mut [Slot; 2]) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_node_layout_invariants() {
        assert_eq!(core::mem::size_of::<Node>(), 0x0100);
        assert_eq!(core::mem::align_of::<Node>(), 8);
        assert_eq!(core::mem::size_of::<[Slot; 2]>(), 0x0100);
    }

    #[test]
    fn ok_node_child_masking() {
        let mut node = Node::empty();
        assert!(node.is_node());
        assert!(node.validate_invariants());

        node.set_child(0, 0x8000_0005);
        assert_eq!(node.get_child(0), 5);
        assert!(node.is_node());

        node.set_child(0x3F, 0xFFFF_FFFF);
        assert_eq!(node.get_child(0x3F), 0x7FFF_FFFF);
        assert!(node.validate_invariants());
    }

    #[test]
    fn ok_node_cyclic_child_index_and_routing() {
        let hash: u32 = 0b000001_000010_000011_000100_000101_000110;

        assert_eq!(Node::child_index(hash, 0), 0b000110);
        assert_eq!(Node::child_index(hash, 1), 0b000101);
        assert_eq!(Node::child_index(hash, 2), 0b000100);
        assert_eq!(Node::child_index(hash, 3), 0b000011);
        assert_eq!(Node::child_index(hash, 4), 0b000010);

        let wrapped_depth = 0x20 / 6;
        let expected_idx = Node::child_index(hash, 0);
        assert_eq!(Node::child_index(hash, 0x20), expected_idx);

        let mut node = Node::empty();
        node.set_child(0b000110, 0x0A);
        node.set_child(0b000101, 0x0B);

        assert_eq!(node.route_child(hash, 0), 0x0A);
        assert_eq!(node.route_child(hash, 1), 0x0B);
        assert_eq!(node.route_child(hash, 2), 0);
        assert_eq!(wrapped_depth, 5);
    }

    #[test]
    fn ok_node_occupancy_and_mutators() {
        let mut node = Node::empty();
        assert!(node.is_empty());
        assert!(!node.is_full());
        assert_eq!(node.occupied_count(), 0);

        assert!(node.is_child_empty(0));
        assert_eq!(node.child_vp(0), None);

        node.set_child(0, 0x14);
        assert!(!node.is_empty());
        assert!(!node.is_child_empty(0));
        assert_eq!(node.child_vp(0), Some(0x14));
        assert_eq!(node.occupied_count(), 1);

        node.clear_child(0);
        assert!(node.is_empty());
        assert_eq!(node.child_vp(0), None);

        node.populate_range(4);
        assert!(node.is_full());
        assert_eq!(node.occupied_count(), 0x40);
        assert_eq!(node.get_child(0), 4);
        assert_eq!(node.get_child(0x3F), 4 + 0x3F);

        let collected: Vec<(usize, u32)> = node.iter_children().collect();
        assert_eq!(collected.len(), 0x40);
        assert_eq!(collected[0], (0, 4));
        assert_eq!(collected[0x3F], (0x3F, 4 + 0x3F));
    }

    #[test]
    fn ok_node_init_from_contiguous() {
        let node = Node::init_from_contiguous(0x04);
        assert!(node.is_full());
        assert_eq!(node.occupied_count(), 0x40);
        for i in 0..0x40 {
            assert_eq!(node.get_child(i), 0x04 + (i as u32));
        }
    }

    #[test]
    fn ok_node_slot_and_bytes_casts() {
        let mut slots = [Slot::zeroed(); 2];
        {
            let node_mut = Node::from_slots_mut(&mut slots);
            node_mut.set_child(0, 0x2A);
            node_mut.set_child(1, 0x2B);
        }

        let node_ref = Node::from_slots(&slots);
        assert_eq!(node_ref.get_child(0), 0x2A);
        assert_eq!(node_ref.get_child(1), 0x2B);

        let back_to_slots = node_ref.as_slots();
        assert_eq!(back_to_slots.len(), 2);

        let bytes = node_ref.as_bytes();
        assert_eq!(bytes.len(), 0x0100);
        assert_eq!(u32::from_ne_bytes(bytes[0..4].try_into().unwrap()), 0x2A);
    }
}
