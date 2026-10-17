//! Ref: `DESIGN.md` §1, §3

use core::ops::{Deref, DerefMut};

#[repr(C, align(8))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Slot(pub(crate) [u8; 0x80]);

const _: () = assert!(core::mem::size_of::<Slot>() == 0x80);
const _: () = assert!(core::mem::align_of::<Slot>() == 8);

impl Default for Slot {
    #[inline(always)]
    fn default() -> Self {
        Self::zeroed()
    }
}

impl Deref for Slot {
    type Target = [u8; Self::SIZE];

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

impl Slot {
    pub(crate) const SIZE: usize = 0x80;
    pub(crate) const ALIGN: usize = 8;
    pub(crate) const SLOTS_PER_VP: usize = 2;
    pub(crate) const SLOTS_PER_OS_PAGE: usize = 0x20;

    #[inline(always)]
    pub(crate) const fn zeroed() -> Self {
        Self([0u8; 0x80])
    }

    #[inline(always)]
    pub(crate) fn is_zeroed(&self) -> bool {
        self.0.iter().all(|&b| b == 0)
    }

    #[inline(always)]
    pub(crate) fn clear(&mut self) {
        self.0.fill(0);
    }

    #[inline(always)]
    pub(crate) fn as_bytes(&self) -> &[u8; Self::SIZE] {
        &self.0
    }

    #[inline(always)]
    pub(crate) fn as_bytes_mut(&mut self) -> &mut [u8; Self::SIZE] {
        &mut self.0
    }

    #[inline(always)]
    pub(crate) fn from_bytes(bytes: &[u8; Self::SIZE]) -> &Self {
        debug_assert_eq!(bytes.as_ptr().align_offset(8), 0);
        unsafe { &*(bytes.as_ptr() as *const Self) }
    }

    #[inline(always)]
    pub(crate) fn from_bytes_mut(bytes: &mut [u8; Self::SIZE]) -> &mut Self {
        debug_assert_eq!(bytes.as_mut_ptr().align_offset(8), 0);
        unsafe { &mut *(bytes.as_mut_ptr() as *mut Self) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_slot_layout_invariants() {
        assert_eq!(core::mem::size_of::<Slot>(), 0x80);
        assert_eq!(core::mem::align_of::<Slot>(), 8);
    }

    #[test]
    fn ok_slot_zeroed_and_clear() {
        let mut s = Slot::zeroed();
        assert_eq!(s.0, [0u8; 0x80]);
        assert_eq!(Slot::default(), s);
        assert!(s.is_zeroed());

        s[0] = 0xFF;
        assert!(!s.is_zeroed());
        assert_eq!(s[0], 0xFF);

        s.clear();
        assert!(s.is_zeroed());
    }

    #[test]
    fn ok_slot_byte_views_and_casts() {
        let mut raw = [0xAAu8; 0x80];
        let slot_ref = Slot::from_bytes(&raw);
        assert_eq!(slot_ref.as_bytes()[0], 0xAA);

        let slot_mut = Slot::from_bytes_mut(&mut raw);
        slot_mut.as_bytes_mut()[0] = 0x55;
        assert_eq!(raw[0], 0x55);
    }
}
