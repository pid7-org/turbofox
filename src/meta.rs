//! TurboFox database header metadata layout (Slot 0)
//!
//! Ref: `DESIGN.md` §2, §3.1, §5.1

use crate::slot::Slot;

#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Metadata {
    /// Magic byte signature: 0x54424658 ("TBFX")
    pub(crate) magic: [u8; 4],
    /// Layout version (currently 1)
    pub(crate) version: u32,
    /// Host OS physical page size in bytes (e.g., 0x1000)
    pub(crate) page_size: u32,
    /// Root Node virtual page index (default: 3)
    pub(crate) root_node_vp: u32,
    /// Total allocated physical OS pages
    pub(crate) total_pages: u32,
    /// Total operational slots in the memory map
    pub(crate) total_slots: u32,
    /// Total active key-value and key-list records
    pub(crate) total_entries: u64,
    /// Head block index for Adjacent Array allocation chain
    pub(crate) adj_arr_head: u32,
    /// Head block index for DataBuf allocation chain
    pub(crate) databuf_head: u32,
    /// Last durable sync epoch ticket
    pub(crate) last_epoch: u64,
    /// Reserved for subsequent specification extensions
    pub(crate) reserved: [u8; 0x50],
}

const _: () = assert!(core::mem::size_of::<Metadata>() == 0x80);
const _: () = assert!(core::mem::align_of::<Metadata>() == 8);

impl Default for Metadata {
    #[inline(always)]
    fn default() -> Self {
        Self::new_default()
    }
}

impl Metadata {
    pub(crate) const MAGIC: [u8; 4] = *b"TBFX";
    pub(crate) const CURRENT_VERSION: u32 = 1;
    pub(crate) const DEFAULT_PAGE_SIZE: u32 = 0x1000;
    pub(crate) const DEFAULT_ROOT_NODE_VP: u32 = 3;
    pub(crate) const DEFAULT_INITIAL_PAGES: u32 = 8;
    pub(crate) const DEFAULT_ADJ_ARR_HEAD: u32 = 0;
    pub(crate) const DEFAULT_DATABUF_HEAD: u32 = 0;

    /// Generates initial database metadata per DESIGN.md §2 (8 pages / 0x0100 slots)
    #[inline(always)]
    pub(crate) const fn new_default() -> Self {
        Self::init(Self::DEFAULT_PAGE_SIZE)
    }

    /// Initializes metadata with a specified OS page size
    #[inline(always)]
    pub(crate) const fn init(page_size: u32) -> Self {
        let total_slots = (Self::DEFAULT_INITIAL_PAGES * page_size) / (Slot::SIZE as u32);
        Self {
            magic: Self::MAGIC,
            version: Self::CURRENT_VERSION,
            page_size,
            root_node_vp: Self::DEFAULT_ROOT_NODE_VP,
            total_pages: Self::DEFAULT_INITIAL_PAGES,
            total_slots,
            total_entries: 0,
            adj_arr_head: Self::DEFAULT_ADJ_ARR_HEAD,
            databuf_head: Self::DEFAULT_DATABUF_HEAD,
            last_epoch: 0,
            reserved: [0u8; 0x50],
        }
    }

    /// Validates database header magic, format version, and geometric invariants
    #[inline(always)]
    pub(crate) fn validate(&self) -> bool {
        self.magic == Self::MAGIC
            && self.version == Self::CURRENT_VERSION
            && self.page_size >= Self::DEFAULT_PAGE_SIZE
            && self.page_size.is_power_of_two()
            && self.total_pages >= Self::DEFAULT_INITIAL_PAGES
            && self.total_slots == (self.total_pages * self.page_size) / (Slot::SIZE as u32)
            && self.root_node_vp == Self::DEFAULT_ROOT_NODE_VP
    }

    #[inline(always)]
    pub(crate) fn record_growth(&mut self, added_pages: u32) {
        self.total_pages = self.total_pages.saturating_add(added_pages);
        self.total_slots = (self.total_pages * self.page_size) / (Slot::SIZE as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_metadata_layout_invariants() {
        assert_eq!(core::mem::size_of::<Metadata>(), 0x80);
        assert_eq!(core::mem::align_of::<Metadata>(), 8);
    }

    #[test]
    fn ok_metadata_field_offsets() {
        assert_eq!(core::mem::offset_of!(Metadata, magic), 0);
        assert_eq!(core::mem::offset_of!(Metadata, version), 4);
        assert_eq!(core::mem::offset_of!(Metadata, page_size), 8);
        assert_eq!(core::mem::offset_of!(Metadata, root_node_vp), 0x0C);
        assert_eq!(core::mem::offset_of!(Metadata, total_pages), 0x10);
        assert_eq!(core::mem::offset_of!(Metadata, total_slots), 0x14);
        assert_eq!(core::mem::offset_of!(Metadata, total_entries), 0x18);
        assert_eq!(core::mem::offset_of!(Metadata, adj_arr_head), 0x20);
        assert_eq!(core::mem::offset_of!(Metadata, databuf_head), 0x24);
        assert_eq!(core::mem::offset_of!(Metadata, last_epoch), 0x28);
        assert_eq!(core::mem::offset_of!(Metadata, reserved), 0x30);
    }

    #[test]
    fn ok_metadata_initialization_and_validation() {
        let meta = Metadata::new_default();
        assert!(meta.validate());
        assert_eq!(meta.total_pages, 8);
        assert_eq!(meta.total_slots, 0x0100);
        assert_eq!(meta.total_entries, 0);
        assert_eq!(meta.root_node_vp, 3);
        assert_eq!(meta.version, 1);
        assert_eq!(meta.magic, *b"TBFX");
    }

    #[test]
    fn ok_metadata_mutators() {
        let mut meta = Metadata::new_default();
        meta.record_growth(4);
        assert_eq!(meta.total_pages, 0x0C);
        assert_eq!(meta.total_slots, 0x0180);
        assert!(meta.validate());
    }
}
