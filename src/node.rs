use crate::common::{NODE_CHILDREN, NODE_FLAG, NODE_SIZE, NODE_SLOTS, Slot};

/// Ref: `DESIGN.md` §3.2
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Node {
    pub(crate) flag: u32,
    pub(crate) bucket_indices: [u32; NODE_CHILDREN],
}

const _: () = assert!(core::mem::size_of::<Node>() == NODE_SIZE);
const _: () = assert!(core::mem::align_of::<Node>() == 8);
const _: () = assert!(core::mem::offset_of!(Node, flag) == 0);
const _: () = assert!(core::mem::offset_of!(Node, bucket_indices) == 4);
const _: () = assert!(core::mem::size_of::<[u32; NODE_CHILDREN]>() == 60);
const _: () = assert!(core::mem::size_of::<Node>() == NODE_SLOTS * Slot::SIZE);

impl Default for Node {
    #[inline(always)]
    fn default() -> Self {
        Self::new()
    }
}

impl Node {
    #[inline(always)]
    pub(crate) const fn new() -> Self {
        Self { flag: NODE_FLAG, bucket_indices: [0u32; NODE_CHILDREN] }
    }

    #[inline(always)]
    pub(crate) const fn with_children(children: [u32; NODE_CHILDREN]) -> Self {
        Self { flag: NODE_FLAG, bucket_indices: children }
    }

    #[inline(always)]
    pub(crate) const fn is_child_node(&self, idx: usize) -> bool {
        debug_assert!(idx < NODE_CHILDREN);
        (self.flag & (1 << idx)) != 0
    }

    #[inline(always)]
    pub(crate) fn set_child_node(&mut self, idx: usize) {
        debug_assert!(idx < NODE_CHILDREN);
        self.flag |= 1 << idx;
    }

    #[inline(always)]
    pub(crate) fn as_slot(&self) -> &Slot {
        unsafe { &*core::ptr::from_ref(self).cast() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_node_layout_invariants() {
        assert_eq!(core::mem::size_of::<Node>(), 0x40);
        assert_eq!(core::mem::align_of::<Node>(), 8);
        assert_eq!(core::mem::offset_of!(Node, flag), 0);
        assert_eq!(core::mem::offset_of!(Node, bucket_indices), 4);
    }

    #[test]
    fn ok_node_initialization_and_accessors() {
        let mut node = Node::new();
        assert_eq!(node.flag, NODE_FLAG);
        assert_eq!(node.bucket_indices, [0u32; 15]);

        node.bucket_indices[0] = 0x04;
        node.bucket_indices[0x0E] = 0x3C;
        assert_eq!(node.bucket_indices[0], 0x04);
        assert_eq!(node.bucket_indices[0x0E], 0x3C);

        let slot_ref = node.as_slot();
        assert_eq!(slot_ref[0..4], NODE_FLAG.to_ne_bytes());
    }

    #[test]
    fn ok_node_child_type_bitmap() {
        let mut node = Node::new();
        for i in 0..NODE_CHILDREN {
            assert!(!node.is_child_node(i));
        }

        node.set_child_node(0);
        node.set_child_node(7);
        node.set_child_node(0x0E);

        assert!(node.is_child_node(0));
        assert!(node.is_child_node(7));
        assert!(node.is_child_node(0x0E));
        assert!(!node.is_child_node(1));
        assert!(!node.is_child_node(0x0D));
    }
}
