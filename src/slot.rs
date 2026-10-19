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

    #[inline(always)]
    pub(crate) const fn zeroed() -> Self {
        Self([0u8; 0x80])
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
    fn ok_slot_zeroed() {
        let s = Slot::zeroed();
        assert_eq!(s.0, [0u8; 0x80]);
        assert_eq!(Slot::default(), s);
    }
}
