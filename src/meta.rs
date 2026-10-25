use crate::common::{
    INITIAL_FREE_SLOT_MARKER, LAYOUT_VERSION, MAGIC, METADATA_SIZE, PAGE_SIZE_4K, PAGE_SIZE_16K,
    PAGE_SIZE_64K, SLOT_ALIGN, SLOT_SIZE, Slot,
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
    pub(crate) reserved: [u8; 0x60],
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
const _: () = assert!(core::mem::offset_of!(Metadata, reserved) == 0x20);

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
        let total_slots = (total_pages * page_size) / (SLOT_SIZE as u32);

        Self {
            magic: Self::MAGIC,
            version,
            page_size,
            total_pages,
            total_slots,
            free_slot_marker: INITIAL_FREE_SLOT_MARKER,
            total_entries: 0,
            reserved: [0u8; 0x60],
        }
    }

    /// Ref: `DESIGN.md` §8.1
    #[inline(always)]
    pub(crate) fn validate(&self, expected_page_size: u32, expected_version: u32) -> bool {
        self.magic == Self::MAGIC
            && self.version == expected_version
            && self.page_size == expected_page_size
            && (self.page_size == (PAGE_SIZE_4K as u32)
                || self.page_size == (PAGE_SIZE_16K as u32)
                || self.page_size == (PAGE_SIZE_64K as u32))
            && self.total_pages > 0
            && self.total_slots == (self.total_pages * self.page_size) / (SLOT_SIZE as u32)
            && self.free_slot_marker >= INITIAL_FREE_SLOT_MARKER
            && self.free_slot_marker <= self.total_slots
    }

    /// Ref: `DESIGN.md` §5.4
    #[inline(always)]
    pub(crate) fn record_growth(&mut self, added_pages: u32) {
        self.total_pages = self.total_pages.saturating_add(added_pages);
        self.total_slots = (self.total_pages * self.page_size) / (SLOT_SIZE as u32);
    }

    #[allow(dead_code)]
    #[inline(always)]
    pub(crate) fn as_slot(&self) -> &Slot {
        unsafe { &*(self as *const Self as *const Slot) }
    }

    #[allow(dead_code)]
    #[inline(always)]
    pub(crate) fn as_slot_mut(&mut self) -> &mut Slot {
        unsafe { &mut *(self as *mut Self as *mut Slot) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_metadata_layout_invariants() {
        assert_eq!(core::mem::size_of::<Metadata>(), 0x80);
        assert_eq!(core::mem::align_of::<Metadata>(), 8);
        assert_eq!(core::mem::offset_of!(Metadata, magic), 0);
        assert_eq!(core::mem::offset_of!(Metadata, version), 4);
        assert_eq!(core::mem::offset_of!(Metadata, page_size), 8);
        assert_eq!(core::mem::offset_of!(Metadata, total_pages), 0x0C);
        assert_eq!(core::mem::offset_of!(Metadata, total_slots), 0x10);
        assert_eq!(core::mem::offset_of!(Metadata, free_slot_marker), 0x14);
        assert_eq!(core::mem::offset_of!(Metadata, total_entries), 0x18);
        assert_eq!(core::mem::offset_of!(Metadata, reserved), 0x20);
    }

    #[test]
    fn ok_metadata_initialization_and_validation() {
        let meta = Metadata::new(PAGE_SIZE_4K as u32, 2, 4);
        assert!(meta.validate(PAGE_SIZE_4K as u32, 2));
        assert_eq!(meta.total_pages, 4);
        assert_eq!(meta.total_slots, 0x80);
        assert_eq!(meta.free_slot_marker, 0x20);
        assert_eq!(meta.total_entries, 0);
        assert_eq!(meta.version, 2);
        assert_eq!(meta.magic, *b"TBFX");
    }

    #[test]
    fn ok_metadata_growth_tracking() {
        let mut meta = Metadata::new(PAGE_SIZE_4K as u32, 2, 4);
        meta.record_growth(4);
        assert_eq!(meta.total_pages, 8);
        assert_eq!(meta.total_slots, 0x0100);
        assert!(meta.validate(PAGE_SIZE_4K as u32, 2));
    }

    #[test]
    fn ok_metadata_slot_conversion() {
        let meta = Metadata::new(PAGE_SIZE_4K as u32, 2, 4);
        let slot = meta.as_slot();
        assert_eq!(&slot.0[..4], b"TBFX");
    }

    #[test]
    fn err_metadata_validation_matrix() {
        let valid = Metadata::new(PAGE_SIZE_4K as u32, 2, 4);
        assert!(valid.validate(PAGE_SIZE_4K as u32, 2));

        let mut bad_magic = valid;
        bad_magic.magic = *b"NOPE";
        assert!(!bad_magic.validate(PAGE_SIZE_4K as u32, 2));

        let mut bad_version = valid;
        bad_version.version = 3;
        assert!(!bad_version.validate(PAGE_SIZE_4K as u32, 2));

        let mut bad_page_size = valid;
        bad_page_size.page_size = 0x2000;
        assert!(!bad_page_size.validate(0x2000, 2));

        let mut zero_pages = valid;
        zero_pages.total_pages = 0;
        assert!(!zero_pages.validate(PAGE_SIZE_4K as u32, 2));

        let mut bad_slots = valid;
        bad_slots.total_slots = 0x7F;
        assert!(!bad_slots.validate(PAGE_SIZE_4K as u32, 2));

        let mut bad_marker_low = valid;
        bad_marker_low.free_slot_marker = 0x1F;
        assert!(!bad_marker_low.validate(PAGE_SIZE_4K as u32, 2));

        let mut bad_marker_high = valid;
        bad_marker_high.free_slot_marker = valid.total_slots + 1;
        assert!(!bad_marker_high.validate(PAGE_SIZE_4K as u32, 2));
    }

    #[test]
    fn ok_metadata_page_size_variants() {
        let meta16 = Metadata::new(PAGE_SIZE_16K as u32, 2, 1);
        assert!(meta16.validate(PAGE_SIZE_16K as u32, 2));
        assert_eq!(meta16.total_slots, 0x80);

        let meta64 = Metadata::new(PAGE_SIZE_64K as u32, 2, 1);
        assert!(meta64.validate(PAGE_SIZE_64K as u32, 2));
        assert_eq!(meta64.total_slots, 0x0200);

        let default_meta = Metadata::default();
        assert!(default_meta.validate(PAGE_SIZE_4K as u32, 2));
    }
}
