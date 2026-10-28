use crate::common::{NODE_CHILDREN, NODE_FLAG, NODE_SIZE, NODE_SLOTS, Slot};

/// Ref: `DESIGN.md` §3.1
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Node {
    pub(crate) flag: u32,
    pub(crate) padding: [u8; 4],
    pub(crate) bucket_indices: [u32; NODE_CHILDREN],
}

const _: () = assert!(core::mem::size_of::<Node>() == NODE_SIZE);
const _: () = assert!(core::mem::align_of::<Node>() == 8);
const _: () = assert!(core::mem::offset_of!(Node, flag) == 0);
const _: () = assert!(core::mem::offset_of!(Node, padding) == 4);
const _: () = assert!(core::mem::offset_of!(Node, bucket_indices) == 8);
const _: () = assert!(core::mem::size_of::<[u32; NODE_CHILDREN]>() == 0x78);
const _: () = assert!(core::mem::size_of::<Node>() == NODE_SLOTS * Slot::SIZE);

impl Default for Node {
    #[inline(always)]
    fn default() -> Self {
        Self::new()
    }
}

impl Node {
    #[allow(dead_code)]
    pub(crate) const SIZE: usize = NODE_SIZE;
    #[allow(dead_code)]
    pub(crate) const CHILDREN: usize = NODE_CHILDREN;

    #[inline(always)]
    pub(crate) const fn new() -> Self {
        Self { flag: NODE_FLAG, padding: [0u8; 4], bucket_indices: [0u32; NODE_CHILDREN] }
    }

    #[inline(always)]
    pub(crate) const fn with_children(children: [u32; NODE_CHILDREN]) -> Self {
        Self { flag: NODE_FLAG, padding: [0u8; 4], bucket_indices: children }
    }

    #[allow(dead_code)]
    #[inline(always)]
    pub(crate) fn child_slot(&self, idx: usize) -> u32 {
        debug_assert!(idx < NODE_CHILDREN);
        self.bucket_indices[idx]
    }

    #[allow(dead_code)]
    #[inline(always)]
    pub(crate) fn set_child_slot(&mut self, idx: usize, slot: u32) {
        debug_assert!(idx < NODE_CHILDREN);
        self.bucket_indices[idx] = slot;
    }

    #[inline(always)]
    pub(crate) fn as_slot(&self) -> &Slot {
        unsafe { &*core::ptr::from_ref(self).cast() }
    }

    #[allow(dead_code)]
    #[inline(always)]
    pub(crate) fn as_slot_mut(&mut self) -> &mut Slot {
        unsafe { &mut *core::ptr::from_mut(self).cast() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_node_layout_invariants() {
        assert_eq!(core::mem::size_of::<Node>(), 0x80);
        assert_eq!(core::mem::align_of::<Node>(), 8);
        assert_eq!(core::mem::offset_of!(Node, flag), 0);
        assert_eq!(core::mem::offset_of!(Node, padding), 4);
        assert_eq!(core::mem::offset_of!(Node, bucket_indices), 8);
    }

    #[test]
    fn ok_node_initialization_and_accessors() {
        let mut node = Node::new();
        assert_eq!(node.flag, NODE_FLAG);
        assert_eq!(node.bucket_indices, [0u32; 30]);

        node.set_child_slot(0, 0x11);
        node.set_child_slot(0x1D, 0x2E);
        assert_eq!(node.child_slot(0), 0x11);
        assert_eq!(node.child_slot(0x1D), 0x2E);

        let slot_ref = node.as_slot();
        assert_eq!(slot_ref[0..4], NODE_FLAG.to_ne_bytes());
    }
}
