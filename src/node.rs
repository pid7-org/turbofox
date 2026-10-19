//! Trie branch Node structure
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

    #[inline(always)]
    pub(crate) const fn child_index(hash: u32, depth: u32) -> usize {
        let shift = (depth * Self::BITS_PER_LEVEL) % Self::HASH_BITS;
        ((hash >> shift) & Self::CHILD_INDEX_MASK) as usize
    }

    #[inline(always)]
    pub(crate) fn get_child(&self, idx: usize) -> u32 {
        self.children[idx] & Self::MASK_INDEX
    }

    #[inline(always)]
    pub(crate) fn set_child(&mut self, idx: usize, child_idx: u32) {
        self.children[idx] = child_idx & Self::MASK_INDEX;
    }

    #[inline(always)]
    pub(crate) fn route_child(&self, hash: u32, depth: u32) -> u32 {
        self.get_child(Self::child_index(hash, depth))
    }

    #[inline(always)]
    pub(crate) fn is_node(&self) -> bool {
        (self.children[0] & Self::DISCRIMINATOR_BIT) == 0
    }

    #[inline(always)]
    pub(crate) fn init_from_contiguous(base_vp: u32) -> Self {
        let mut node = Self::empty();
        node.populate_range(base_vp);
        node
    }

    #[inline(always)]
    pub(crate) fn populate_range(&mut self, base_vp: u32) {
        for i in 0..Self::CHILD_COUNT {
            self.set_child(i, (base_vp + (i as u32)) & Self::MASK_INDEX);
        }
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
    fn ok_node_layout_invariants() {
        assert_eq!(core::mem::size_of::<Node>(), 0x0100);
        assert_eq!(core::mem::align_of::<Node>(), 8);
        assert_eq!(core::mem::size_of::<[Slot; 2]>(), 0x0100);
    }

    #[test]
    fn ok_node_child_masking() {
        let mut node = Node::empty();
        assert!(node.is_node());

        node.set_child(0, 0x8000_0005);
        assert_eq!(node.get_child(0), 5);
        assert!(node.is_node());

        node.set_child(0x3F, 0xFFFF_FFFF);
        assert_eq!(node.get_child(0x3F), 0x7FFF_FFFF);
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
    fn ok_node_init_from_contiguous() {
        let node = Node::init_from_contiguous(0x04);
        for i in 0..0x40 {
            assert_eq!(node.get_child(i), 0x04 + (i as u32));
        }
    }

    #[test]
    fn ok_node_as_slots() {
        let mut node = Node::empty();
        node.set_child(0, 0x2A);
        node.set_child(1, 0x2B);

        let back_to_slots = node.as_slots();
        assert_eq!(back_to_slots.len(), 2);
    }
}
