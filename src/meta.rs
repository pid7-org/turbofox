use crate::common::{
    EXTRA_NODE_START_SLOT, INITIAL_EXTRA_NODES, INITIAL_FREE_SLOT_MARKER, LAYOUT_VERSION, MAGIC,
    METADATA_SIZE, PAGE_SIZE_4K, PAGE_SIZE_16K, PAGE_SIZE_64K, SLOT_ALIGN, SLOT_SIZE, Slot,
};

/// Ref: `DESIGN.md` §3.1
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Metadata {
    pub(crate) magic: [u8; 4],
    pub(crate) version: u32,
    pub(crate) page_size: u32,
    pub(crate) total_pages: u32,
    pub(crate) total_slots: u32,
    pub(crate) free_slot_marker: u32,
    pub(crate) total_entries: u64,
    pub(crate) extra_nodes: [u32; 4],
    pub(crate) extra_node_count: u32,
    pub(crate) reserved: [u8; 12],
}

const _: () = assert!(core::mem::size_of::<Metadata>() == METADATA_SIZE);
const _: () = assert!(core::mem::size_of::<Metadata>() == SLOT_SIZE);
const _: () = assert!(core::mem::align_of::<Metadata>() == SLOT_ALIGN);
const _: () = assert!(core::mem::offset_of!(Metadata, magic) == 0);
const _: () = assert!(core::mem::offset_of!(Metadata, version) == 4);
const _: () = assert!(core::mem::offset_of!(Metadata, page_size) == 8);
const _: () = assert!(core::mem::offset_of!(Metadata, total_pages) == 0x0C);
const _: () = assert!(core::mem::offset_of!(Metadata, total_slots) == 0x10);
const _: () = assert!(core::mem::offset_of!(Metadata, free_slot_marker) == 0x14);
const _: () = assert!(core::mem::offset_of!(Metadata, total_entries) == 0x18);
const _: () = assert!(core::mem::offset_of!(Metadata, extra_nodes) == 0x20);
const _: () = assert!(core::mem::offset_of!(Metadata, extra_node_count) == 0x30);
const _: () = assert!(core::mem::offset_of!(Metadata, reserved) == 0x34);

impl Default for Metadata {
    #[inline(always)]
    fn default() -> Self {
        Self::new(PAGE_SIZE_4K as u32, LAYOUT_VERSION, 4)
    }
}

impl Metadata {
    pub(crate) const MAGIC: [u8; 4] = MAGIC;

    /// Ref: `DESIGN.md` §3.1
    #[inline(always)]
    pub(crate) const fn new(page_size: u32, version: u32, total_pages: u32) -> Self {
        let computed = ((total_pages as u64) * (page_size as u64)) / (SLOT_SIZE as u64);
        let total_slots =
            if computed > (0xFFFF_FFFFu64) { 0xFFFF_FFFFu32 } else { computed as u32 };

        Self {
            magic: Self::MAGIC,
            version,
            page_size,
            total_pages,
            total_slots,
            free_slot_marker: INITIAL_FREE_SLOT_MARKER,
            total_entries: 0,
            extra_nodes: [
                EXTRA_NODE_START_SLOT,
                EXTRA_NODE_START_SLOT + 1,
                EXTRA_NODE_START_SLOT + 2,
                EXTRA_NODE_START_SLOT + 3,
            ],
            extra_node_count: INITIAL_EXTRA_NODES as u32,
            reserved: [0u8; 12],
        }
    }

    /// Ref: `DESIGN.md` §8.1
    #[inline(always)]
    pub(crate) fn validate(&self, expected_page_size: u32, expected_version: u32) -> bool {
        let computed = ((self.total_pages as u64) * (self.page_size as u64)) / (SLOT_SIZE as u64);
        let expected_slots =
            if computed > (0xFFFF_FFFFu64) { 0xFFFF_FFFFu32 } else { computed as u32 };

        self.magic == Self::MAGIC
            && self.version == expected_version
            && self.page_size == expected_page_size
            && (self.page_size == (PAGE_SIZE_4K as u32)
                || self.page_size == (PAGE_SIZE_16K as u32)
                || self.page_size == (PAGE_SIZE_64K as u32))
            && self.total_pages > 0
            && self.total_slots == expected_slots
            && self.free_slot_marker >= INITIAL_FREE_SLOT_MARKER
            && self.free_slot_marker <= self.total_slots
            && (self.extra_node_count as usize) <= self.extra_nodes.len()
    }

    /// Ref: `DESIGN.md` §4.4
    #[inline(always)]
    pub(crate) fn pop_extra_node(&mut self) -> Option<u32> {
        if self.extra_node_count > 0 {
            self.extra_node_count -= 1;
            Some(self.extra_nodes[self.extra_node_count as usize])
        } else {
            None
        }
    }

    /// Ref: `DESIGN.md` §4.4
    #[inline(always)]
    pub(crate) fn push_extra_node(&mut self, slot: u32) -> bool {
        if (self.extra_node_count as usize) < self.extra_nodes.len() {
            self.extra_nodes[self.extra_node_count as usize] = slot;
            self.extra_node_count += 1;
            true
        } else {
            false
        }
    }

    /// Ref: `DESIGN.md` §5.4
    #[inline(always)]
    pub(crate) fn record_growth(&mut self, added_pages: u32) {
        self.total_pages = self.total_pages.saturating_add(added_pages);
        let computed = ((self.total_pages as u64) * (self.page_size as u64)) / (SLOT_SIZE as u64);
        self.total_slots =
            if computed > (0xFFFF_FFFFu64) { 0xFFFF_FFFFu32 } else { computed as u32 };
    }

    #[allow(dead_code)]
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
    fn ok_metadata_layout_invariants() {
        assert_eq!(core::mem::size_of::<Metadata>(), 0x40);
        assert_eq!(core::mem::align_of::<Metadata>(), 8);
        assert_eq!(core::mem::offset_of!(Metadata, magic), 0);
        assert_eq!(core::mem::offset_of!(Metadata, version), 4);
        assert_eq!(core::mem::offset_of!(Metadata, page_size), 8);
        assert_eq!(core::mem::offset_of!(Metadata, total_pages), 0x0C);
        assert_eq!(core::mem::offset_of!(Metadata, total_slots), 0x10);
        assert_eq!(core::mem::offset_of!(Metadata, free_slot_marker), 0x14);
        assert_eq!(core::mem::offset_of!(Metadata, total_entries), 0x18);
        assert_eq!(core::mem::offset_of!(Metadata, extra_nodes), 0x20);
        assert_eq!(core::mem::offset_of!(Metadata, extra_node_count), 0x30);
        assert_eq!(core::mem::offset_of!(Metadata, reserved), 0x34);
    }

    #[test]
    fn ok_metadata_initialization_and_validation() {
        let meta = Metadata::new(PAGE_SIZE_4K as u32, LAYOUT_VERSION, 4);
        assert!(meta.validate(PAGE_SIZE_4K as u32, LAYOUT_VERSION));
        assert_eq!(meta.total_pages, 4);
        assert_eq!(meta.total_slots, 0x0100);
        assert_eq!(meta.free_slot_marker, INITIAL_FREE_SLOT_MARKER);
        assert_eq!(meta.total_entries, 0);
        assert_eq!(meta.extra_node_count, 4);
        assert_eq!(meta.extra_nodes, [124, 125, 126, 127]);
        assert_eq!(meta.version, LAYOUT_VERSION);
        assert_eq!(meta.magic, *b"TBFX");
    }

    #[test]
    fn ok_metadata_extra_nodes_stack() {
        let mut meta = Metadata::new(PAGE_SIZE_4K as u32, LAYOUT_VERSION, 4);
        assert_eq!(meta.pop_extra_node(), Some(127));
        assert_eq!(meta.pop_extra_node(), Some(126));
        assert_eq!(meta.pop_extra_node(), Some(125));
        assert_eq!(meta.pop_extra_node(), Some(124));
        assert_eq!(meta.pop_extra_node(), None);
        assert_eq!(meta.extra_node_count, 0);

        assert!(meta.push_extra_node(200));
        assert_eq!(meta.extra_node_count, 1);
        assert_eq!(meta.pop_extra_node(), Some(200));
    }

    #[test]
    fn ok_metadata_growth_tracking() {
        let mut meta = Metadata::new(PAGE_SIZE_4K as u32, LAYOUT_VERSION, 4);
        meta.record_growth(4);
        assert_eq!(meta.total_pages, 8);
        assert_eq!(meta.total_slots, 0x0200);
        assert!(meta.validate(PAGE_SIZE_4K as u32, LAYOUT_VERSION));

        // Growth past 4 GiB (0x0010_0000 pages = 4 GiB)
        meta.record_growth(0x0010_0000);
        assert_eq!(meta.total_pages, 0x0010_0008);
        assert_eq!(meta.total_slots, 0x0010_0008 * 0x40);
        assert!(meta.validate(PAGE_SIZE_4K as u32, LAYOUT_VERSION));

        // Growth up to 256 GiB ceiling (0x0400_0000 4K pages) saturates total_slots to 0xFFFF_FFFF
        let max_meta = Metadata::new(PAGE_SIZE_4K as u32, LAYOUT_VERSION, 0x0400_0000);
        assert_eq!(max_meta.total_slots, 0xFFFF_FFFF);
        assert!(max_meta.validate(PAGE_SIZE_4K as u32, LAYOUT_VERSION));
    }

    #[test]
    fn ok_metadata_slot_conversion() {
        let meta = Metadata::new(PAGE_SIZE_4K as u32, LAYOUT_VERSION, 4);
        let slot = meta.as_slot();
        assert_eq!(&slot.0[..4], b"TBFX");
    }

    #[test]
    fn err_metadata_validation_matrix() {
        let valid = Metadata::new(PAGE_SIZE_4K as u32, LAYOUT_VERSION, 4);
        assert!(valid.validate(PAGE_SIZE_4K as u32, LAYOUT_VERSION));

        let mut bad_magic = valid;
        bad_magic.magic = *b"NOPE";
        assert!(!bad_magic.validate(PAGE_SIZE_4K as u32, LAYOUT_VERSION));

        let mut bad_version = valid;
        bad_version.version = 0xFF;
        assert!(!bad_version.validate(PAGE_SIZE_4K as u32, LAYOUT_VERSION));

        let mut bad_page_size = valid;
        bad_page_size.page_size = 0x2000;
        assert!(!bad_page_size.validate(0x2000, LAYOUT_VERSION));

        let mut zero_pages = valid;
        zero_pages.total_pages = 0;
        assert!(!zero_pages.validate(PAGE_SIZE_4K as u32, LAYOUT_VERSION));

        let mut bad_slots = valid;
        bad_slots.total_slots = 0x7F;
        assert!(!bad_slots.validate(PAGE_SIZE_4K as u32, LAYOUT_VERSION));

        let mut bad_marker_low = valid;
        bad_marker_low.free_slot_marker = INITIAL_FREE_SLOT_MARKER - 1;
        assert!(!bad_marker_low.validate(PAGE_SIZE_4K as u32, LAYOUT_VERSION));

        let mut bad_marker_high = valid;
        bad_marker_high.free_slot_marker = valid.total_slots + 1;
        assert!(!bad_marker_high.validate(PAGE_SIZE_4K as u32, LAYOUT_VERSION));
    }

    #[test]
    fn ok_metadata_page_size_variants() {
        let meta16 = Metadata::new(PAGE_SIZE_16K as u32, LAYOUT_VERSION, 1);
        assert!(meta16.validate(PAGE_SIZE_16K as u32, LAYOUT_VERSION));
        assert_eq!(meta16.total_slots, 0x0100);

        let meta64 = Metadata::new(PAGE_SIZE_64K as u32, LAYOUT_VERSION, 1);
        assert!(meta64.validate(PAGE_SIZE_64K as u32, LAYOUT_VERSION));
        assert_eq!(meta64.total_slots, 0x0400);

        let default_meta = Metadata::default();
        assert!(default_meta.validate(PAGE_SIZE_4K as u32, LAYOUT_VERSION));
    }
}
