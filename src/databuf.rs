//! DataBuf storage layout (0x0400 Bytes / 8 Slots)
//!
//! Ref: `DESIGN.md` §1, §3.5, §5.3, §7

use crate::slot::Slot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DataBufError {
    OutOfSpace,
    InvalidIndex,
    PayloadTooLarge,
    CrcMismatch { expected: u32, found: u32 },
}

#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DataBuf {
    /// 0x3F payload buffers of 0x10 bytes each (0x3F * 0x10 = 0x03F0 bytes)
    pub buffers: [[u8; 0x10]; 0x3F],
    /// Bits 0..0x3E: buffer occupancy (1 = occupied, 0 = free); bit 0x3F: full flag
    pub bitmap: u64,
    /// 0x1F-bit index to next DataBuf in the global allocation chain
    pub next_dbuf: u32,
    /// Alignment padding to exactly 0x0400 bytes
    pub padding: [u8; 4],
}

const _: () = assert!(core::mem::size_of::<DataBuf>() == 0x0400);
const _: () = assert!(core::mem::align_of::<DataBuf>() == 8);

impl Default for DataBuf {
    #[inline(always)]
    fn default() -> Self {
        Self::new()
    }
}

impl DataBuf {
    pub const SIZE: usize = 0x0400;
    pub const SLOT_COUNT: usize = 8;
    pub const BUF_COUNT: usize = 0x3F;
    pub const BUF_SIZE: usize = 0x10;
    pub const PAYLOAD_BYTES: usize = 0x03F0;
    pub const CRC_SIZE: usize = 4;
    pub const FULL_BIT: u64 = 0x8000_0000_0000_0000;
    pub const OCCUPANCY_MASK: u64 = 0x7FFF_FFFF_FFFF_FFFF;
    pub const NEXT_DBUF_MASK: u32 = 0x7FFF_FFFF;

    #[inline(always)]
    pub const fn new() -> Self {
        Self { buffers: [[0u8; 0x10]; 0x3F], bitmap: 0, next_dbuf: 0, padding: [0u8; 4] }
    }

    #[inline(always)]
    pub fn is_full(&self) -> bool {
        (self.bitmap & Self::FULL_BIT) != 0
            || (self.bitmap & Self::OCCUPANCY_MASK) == Self::OCCUPANCY_MASK
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        (self.bitmap & Self::OCCUPANCY_MASK) == 0
    }

    #[inline(always)]
    pub fn occupied_count(&self) -> usize {
        (self.bitmap & Self::OCCUPANCY_MASK).count_ones() as usize
    }

    #[inline(always)]
    pub fn free_count(&self) -> usize {
        Self::BUF_COUNT - self.occupied_count()
    }

    #[inline(always)]
    pub fn is_buffer_occupied(&self, idx: usize) -> bool {
        debug_assert!(idx < Self::BUF_COUNT);
        (self.bitmap & (1u64 << idx)) != 0
    }

    #[inline(always)]
    pub fn set_buffer_occupied(&mut self, idx: usize, occupied: bool) {
        debug_assert!(idx < Self::BUF_COUNT);
        if occupied {
            self.bitmap |= 1u64 << idx;
        } else {
            self.bitmap &= !(1u64 << idx);
        }
        if (self.bitmap & Self::OCCUPANCY_MASK) == Self::OCCUPANCY_MASK {
            self.bitmap |= Self::FULL_BIT;
        } else {
            self.bitmap &= !Self::FULL_BIT;
        }
    }

    #[inline(always)]
    pub(crate) fn is_range_free(&self, start_idx: usize, count: usize) -> bool {
        if count == 0 || start_idx + count > Self::BUF_COUNT {
            return false;
        }
        let mask = if count == 0x40 { u64::MAX } else { ((1u64 << count) - 1) << start_idx };
        (self.bitmap & mask) == 0
    }

    pub(crate) fn find_contiguous_free(&self, count: usize) -> Option<usize> {
        if count == 0 || count > Self::BUF_COUNT || self.is_full() {
            return None;
        }
        let max_start = Self::BUF_COUNT - count;
        let test_mask = if count == 0x40 { u64::MAX } else { (1u64 << count) - 1 };

        for idx in 0..=max_start {
            if (self.bitmap & (test_mask << idx)) == 0 {
                return Some(idx);
            }
        }
        None
    }

    pub(crate) fn alloc_contiguous(&mut self, count: usize) -> Option<usize> {
        let start = self.find_contiguous_free(count)?;
        let mask = if count == 0x40 { u64::MAX } else { ((1u64 << count) - 1) << start };
        self.bitmap |= mask;
        if (self.bitmap & Self::OCCUPANCY_MASK) == Self::OCCUPANCY_MASK {
            self.bitmap |= Self::FULL_BIT;
        }
        Some(start)
    }

    pub(crate) fn free_contiguous(&mut self, start_idx: usize, count: usize) {
        debug_assert!(start_idx + count <= Self::BUF_COUNT);
        if count == 0 {
            return;
        }
        let mask = if count == 0x40 { u64::MAX } else { ((1u64 << count) - 1) << start_idx };
        self.bitmap &= !mask;
        self.bitmap &= !Self::FULL_BIT;
    }

    #[inline(always)]
    pub const fn required_buffers(payload_len: usize) -> usize {
        (Self::CRC_SIZE + payload_len + 0x0F) / Self::BUF_SIZE
    }

    #[inline(always)]
    pub const fn required_kv_buffers(klen: usize, vlen: usize) -> usize {
        Self::required_buffers(klen + vlen)
    }

    #[inline(always)]
    pub const fn required_key_buffers(klen: usize) -> usize {
        Self::required_buffers(klen)
    }

    #[inline(always)]
    pub const fn required_val_buffers(vlen: usize) -> usize {
        Self::required_buffers(vlen)
    }

    #[inline(always)]
    pub const fn global_buf_idx(dbuf_idx: u32, local_idx: usize) -> u32 {
        dbuf_idx * (Self::BUF_COUNT as u32) + (local_idx as u32)
    }

    #[inline(always)]
    pub const fn decompose_buf_idx(global_idx: u32) -> (u32, usize) {
        let dbuf_idx = global_idx / (Self::BUF_COUNT as u32);
        let local_idx = (global_idx % (Self::BUF_COUNT as u32)) as usize;
        (dbuf_idx, local_idx)
    }

    #[inline(always)]
    pub(crate) fn payload_bytes(&self) -> &[u8; Self::PAYLOAD_BYTES] {
        unsafe { &*(self.buffers.as_ptr() as *const [u8; Self::PAYLOAD_BYTES]) }
    }

    #[inline(always)]
    pub(crate) fn payload_bytes_mut(&mut self) -> &mut [u8; Self::PAYLOAD_BYTES] {
        unsafe { &mut *(self.buffers.as_mut_ptr() as *mut [u8; Self::PAYLOAD_BYTES]) }
    }

    pub(crate) fn write_kv_payload(
        &mut self,
        start_idx: usize,
        key: &[u8],
        val: &[u8],
    ) -> Result<(), DataBufError> {
        let req = Self::required_kv_buffers(key.len(), val.len());
        if req > Self::BUF_COUNT || start_idx + req > Self::BUF_COUNT {
            return Err(DataBufError::PayloadTooLarge);
        }

        let start_byte = start_idx * Self::BUF_SIZE;
        let total_bytes = req * Self::BUF_SIZE;
        let data_len = key.len() + val.len();

        let raw = self.payload_bytes_mut();
        let frame = &mut raw[start_byte..start_byte + total_bytes];

        frame[Self::CRC_SIZE..Self::CRC_SIZE + key.len()].copy_from_slice(key);
        frame[Self::CRC_SIZE + key.len()..Self::CRC_SIZE + data_len].copy_from_slice(val);
        frame[Self::CRC_SIZE + data_len..].fill(0);

        let crc = turbo_crc::crc32c(&frame[Self::CRC_SIZE..Self::CRC_SIZE + data_len]);
        frame[..Self::CRC_SIZE].copy_from_slice(&crc.to_le_bytes());

        Ok(())
    }

    pub(crate) fn read_kv_payload<'a>(
        &'a self,
        start_idx: usize,
        klen: usize,
        vlen: usize,
    ) -> Result<(&'a [u8], &'a [u8]), DataBufError> {
        let req = Self::required_kv_buffers(klen, vlen);
        if req > Self::BUF_COUNT || start_idx + req > Self::BUF_COUNT {
            return Err(DataBufError::InvalidIndex);
        }

        let start_byte = start_idx * Self::BUF_SIZE;
        let data_len = klen + vlen;
        let raw = self.payload_bytes();
        let frame = &raw[start_byte..start_byte + (req * Self::BUF_SIZE)];

        let mut crc_bytes = [0u8; Self::CRC_SIZE];
        crc_bytes.copy_from_slice(&frame[..Self::CRC_SIZE]);
        let expected_crc = u32::from_le_bytes(crc_bytes);

        let data = &frame[Self::CRC_SIZE..Self::CRC_SIZE + data_len];
        let actual_crc = turbo_crc::crc32c(data);

        if actual_crc != expected_crc {
            return Err(DataBufError::CrcMismatch { expected: expected_crc, found: actual_crc });
        }

        let key = &data[..klen];
        let val = &data[klen..klen + vlen];
        Ok((key, val))
    }

    pub(crate) fn write_key_payload(
        &mut self,
        start_idx: usize,
        key: &[u8],
    ) -> Result<(), DataBufError> {
        let req = Self::required_key_buffers(key.len());
        if req > Self::BUF_COUNT || start_idx + req > Self::BUF_COUNT {
            return Err(DataBufError::PayloadTooLarge);
        }

        let start_byte = start_idx * Self::BUF_SIZE;
        let total_bytes = req * Self::BUF_SIZE;

        let raw = self.payload_bytes_mut();
        let frame = &mut raw[start_byte..start_byte + total_bytes];

        frame[Self::CRC_SIZE..Self::CRC_SIZE + key.len()].copy_from_slice(key);
        frame[Self::CRC_SIZE + key.len()..].fill(0);

        let crc = turbo_crc::crc32c(key);
        frame[..Self::CRC_SIZE].copy_from_slice(&crc.to_le_bytes());

        Ok(())
    }

    pub(crate) fn read_key_payload<'a>(
        &'a self,
        start_idx: usize,
        klen: usize,
    ) -> Result<&'a [u8], DataBufError> {
        let req = Self::required_key_buffers(klen);
        if req > Self::BUF_COUNT || start_idx + req > Self::BUF_COUNT {
            return Err(DataBufError::InvalidIndex);
        }

        let start_byte = start_idx * Self::BUF_SIZE;
        let raw = self.payload_bytes();
        let frame = &raw[start_byte..start_byte + (req * Self::BUF_SIZE)];

        let mut crc_bytes = [0u8; Self::CRC_SIZE];
        crc_bytes.copy_from_slice(&frame[..Self::CRC_SIZE]);
        let expected_crc = u32::from_le_bytes(crc_bytes);

        let key = &frame[Self::CRC_SIZE..Self::CRC_SIZE + klen];
        let actual_crc = turbo_crc::crc32c(key);

        if actual_crc != expected_crc {
            return Err(DataBufError::CrcMismatch { expected: expected_crc, found: actual_crc });
        }

        Ok(key)
    }

    pub(crate) fn write_val_payload(
        &mut self,
        start_idx: usize,
        val: &[u8],
    ) -> Result<(), DataBufError> {
        let req = Self::required_val_buffers(val.len());
        if req > Self::BUF_COUNT || start_idx + req > Self::BUF_COUNT {
            return Err(DataBufError::PayloadTooLarge);
        }

        let start_byte = start_idx * Self::BUF_SIZE;
        let total_bytes = req * Self::BUF_SIZE;

        let raw = self.payload_bytes_mut();
        let frame = &mut raw[start_byte..start_byte + total_bytes];

        frame[Self::CRC_SIZE..Self::CRC_SIZE + val.len()].copy_from_slice(val);
        frame[Self::CRC_SIZE + val.len()..].fill(0);

        let crc = turbo_crc::crc32c(val);
        frame[..Self::CRC_SIZE].copy_from_slice(&crc.to_le_bytes());

        Ok(())
    }

    pub(crate) fn read_val_payload<'a>(
        &'a self,
        start_idx: usize,
        vlen: usize,
    ) -> Result<&'a [u8], DataBufError> {
        let req = Self::required_val_buffers(vlen);
        if req > Self::BUF_COUNT || start_idx + req > Self::BUF_COUNT {
            return Err(DataBufError::InvalidIndex);
        }

        let start_byte = start_idx * Self::BUF_SIZE;
        let raw = self.payload_bytes();
        let frame = &raw[start_byte..start_byte + (req * Self::BUF_SIZE)];

        let mut crc_bytes = [0u8; Self::CRC_SIZE];
        crc_bytes.copy_from_slice(&frame[..Self::CRC_SIZE]);
        let expected_crc = u32::from_le_bytes(crc_bytes);

        let val = &frame[Self::CRC_SIZE..Self::CRC_SIZE + vlen];
        let actual_crc = turbo_crc::crc32c(val);

        if actual_crc != expected_crc {
            return Err(DataBufError::CrcMismatch { expected: expected_crc, found: actual_crc });
        }

        Ok(val)
    }

    #[inline(always)]
    pub(crate) fn verify_key(&self, start_idx: usize, key: &[u8]) -> bool {
        let req = Self::required_key_buffers(key.len());
        if req > Self::BUF_COUNT || start_idx + req > Self::BUF_COUNT {
            return false;
        }

        let start_byte = start_idx * Self::BUF_SIZE + Self::CRC_SIZE;
        let raw = self.payload_bytes();
        &raw[start_byte..start_byte + key.len()] == key
    }

    #[inline(always)]
    pub fn next_dbuf_index(&self) -> u32 {
        self.next_dbuf & Self::NEXT_DBUF_MASK
    }

    #[inline(always)]
    pub fn set_next_dbuf_index(&mut self, idx: u32) {
        self.next_dbuf = idx & Self::NEXT_DBUF_MASK;
    }

    #[inline(always)]
    pub fn clear(&mut self) {
        *self = Self::new();
    }

    #[inline(always)]
    pub(crate) fn as_slots(&self) -> &[Slot; Self::SLOT_COUNT] {
        unsafe { &*(self as *const Self as *const [Slot; Self::SLOT_COUNT]) }
    }

    #[inline(always)]
    pub(crate) fn as_slots_mut(&mut self) -> &mut [Slot; Self::SLOT_COUNT] {
        unsafe { &mut *(self as *mut Self as *mut [Slot; Self::SLOT_COUNT]) }
    }

    #[inline(always)]
    pub(crate) fn from_slots(slots: &[Slot; Self::SLOT_COUNT]) -> &Self {
        unsafe { &*(slots.as_ptr() as *const Self) }
    }

    #[inline(always)]
    pub(crate) fn from_slots_mut(slots: &mut [Slot; Self::SLOT_COUNT]) -> &mut Self {
        unsafe { &mut *(slots.as_mut_ptr() as *mut Self) }
    }

    #[inline(always)]
    pub(crate) fn as_bytes(&self) -> &[u8; Self::SIZE] {
        unsafe { &*(self as *const Self as *const [u8; Self::SIZE]) }
    }

    #[inline(always)]
    pub(crate) fn as_bytes_mut(&mut self) -> &mut [u8; Self::SIZE] {
        unsafe { &mut *(self as *mut Self as *mut [u8; Self::SIZE]) }
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
    fn ok_databuf_layout_invariants() {
        assert_eq!(core::mem::size_of::<DataBuf>(), 0x0400);
        assert_eq!(core::mem::align_of::<DataBuf>(), 8);
    }

    #[test]
    fn ok_databuf_field_offsets() {
        assert_eq!(core::mem::offset_of!(DataBuf, buffers), 0);
        assert_eq!(core::mem::offset_of!(DataBuf, bitmap), 0x03F0);
        assert_eq!(core::mem::offset_of!(DataBuf, next_dbuf), 0x03F8);
        assert_eq!(core::mem::offset_of!(DataBuf, padding), 0x03FC);
    }

    #[test]
    fn ok_databuf_occupancy_bitmap() {
        let mut dbuf = DataBuf::new();
        assert!(!dbuf.is_full());
        assert!(dbuf.is_empty());
        assert_eq!(dbuf.occupied_count(), 0);
        assert_eq!(dbuf.free_count(), 0x3F);
        assert!(!dbuf.is_buffer_occupied(0));

        dbuf.set_buffer_occupied(0, true);
        assert!(dbuf.is_buffer_occupied(0));
        assert!(!dbuf.is_buffer_occupied(1));
        assert!(!dbuf.is_full());
        assert!(!dbuf.is_empty());
        assert_eq!(dbuf.occupied_count(), 1);
        assert_eq!(dbuf.free_count(), 0x3E);

        dbuf.set_buffer_occupied(0, false);
        assert!(!dbuf.is_buffer_occupied(0));
        assert!(dbuf.is_empty());

        for i in 0..DataBuf::BUF_COUNT {
            dbuf.set_buffer_occupied(i, true);
        }
        assert!(dbuf.is_full());
        assert_eq!(dbuf.occupied_count(), 0x3F);
        assert_eq!(dbuf.free_count(), 0);

        dbuf.clear();
        assert!(dbuf.is_empty());
        assert_eq!(dbuf.free_count(), 0x3F);
    }

    #[test]
    fn ok_databuf_contiguous_alloc_and_free() {
        let mut dbuf = DataBuf::new();
        assert_eq!(dbuf.find_contiguous_free(0x10), Some(0));

        let alloc1 = dbuf.alloc_contiguous(4);
        assert_eq!(alloc1, Some(0));
        assert_eq!(dbuf.occupied_count(), 4);

        let alloc2 = dbuf.alloc_contiguous(8);
        assert_eq!(alloc2, Some(4));
        assert_eq!(dbuf.occupied_count(), 0x0C);

        assert!(dbuf.is_range_free(0x0C, 4));
        assert!(!dbuf.is_range_free(2, 4));

        dbuf.free_contiguous(0, 4);
        assert_eq!(dbuf.occupied_count(), 8);
        assert!(dbuf.is_range_free(0, 4));
        assert!(!dbuf.is_range_free(4, 4));

        assert_eq!(dbuf.find_contiguous_free(4), Some(0));
        assert_eq!(dbuf.find_contiguous_free(0x10), Some(0x0C));

        dbuf.free_contiguous(4, 8);
        assert!(dbuf.is_empty());
    }

    #[test]
    fn ok_databuf_sizing_and_indexing() {
        assert_eq!(DataBuf::required_buffers(0), 1);
        assert_eq!(DataBuf::required_buffers(0x0C), 1);
        assert_eq!(DataBuf::required_buffers(0x0D), 2);
        assert_eq!(DataBuf::required_kv_buffers(4, 8), 1);
        assert_eq!(DataBuf::required_kv_buffers(4, 9), 2);

        let (dbuf_idx, local_idx) = DataBuf::decompose_buf_idx(0x80);
        assert_eq!(dbuf_idx, 2);
        assert_eq!(local_idx, 2);
        assert_eq!(DataBuf::global_buf_idx(dbuf_idx, local_idx), 0x80);
    }

    #[test]
    fn ok_databuf_payload_framing_roundtrip() {
        let mut dbuf = DataBuf::new();
        let key = b"alpha_key";
        let val = b"beta_value_bytes_here";

        let req = DataBuf::required_kv_buffers(key.len(), val.len());
        let start = dbuf.alloc_contiguous(req).unwrap();

        dbuf.write_kv_payload(start, key, val).unwrap();
        assert!(dbuf.verify_key(start, key));
        assert!(!dbuf.verify_key(start, b"wrong_key"));

        let (read_k, read_v) = dbuf.read_kv_payload(start, key.len(), val.len()).unwrap();
        assert_eq!(read_k, key);
        assert_eq!(read_v, val);
    }

    #[test]
    fn err_databuf_crc_corruption() {
        let mut dbuf = DataBuf::new();
        let key = b"secure_key";
        let val = b"secure_data";

        let req = DataBuf::required_kv_buffers(key.len(), val.len());
        let start = dbuf.alloc_contiguous(req).unwrap();
        dbuf.write_kv_payload(start, key, val).unwrap();

        let raw = dbuf.payload_bytes_mut();
        raw[start * DataBuf::BUF_SIZE + DataBuf::CRC_SIZE] ^= 0xFF;

        let res = dbuf.read_kv_payload(start, key.len(), val.len());
        assert!(matches!(res, Err(DataBufError::CrcMismatch { .. })));
    }

    #[test]
    fn ok_databuf_key_and_val_payloads() {
        let mut dbuf = DataBuf::new();
        let key = b"list_root_key";
        let val = b"list_item_entry";

        let start_k = dbuf.alloc_contiguous(DataBuf::required_key_buffers(key.len())).unwrap();
        dbuf.write_key_payload(start_k, key).unwrap();
        assert_eq!(dbuf.read_key_payload(start_k, key.len()).unwrap(), key);

        let start_v = dbuf.alloc_contiguous(DataBuf::required_val_buffers(val.len())).unwrap();
        dbuf.write_val_payload(start_v, val).unwrap();
        assert_eq!(dbuf.read_val_payload(start_v, val.len()).unwrap(), val);
    }

    #[test]
    fn ok_databuf_slot_and_byte_views() {
        let mut dbuf = DataBuf::new();
        dbuf.set_next_dbuf_index(0x42);

        let slots = dbuf.as_slots();
        assert_eq!(slots.len(), 8);

        let restored = DataBuf::from_slots(slots);
        assert_eq!(restored.next_dbuf_index(), 0x42);

        let slots_mut = dbuf.as_slots_mut();
        let restored_mut = DataBuf::from_slots_mut(slots_mut);
        restored_mut.set_next_dbuf_index(0x43);
        assert_eq!(dbuf.next_dbuf_index(), 0x43);

        let bytes = dbuf.as_bytes();
        assert_eq!(bytes.len(), 0x0400);

        let restored_b = DataBuf::from_bytes(bytes);
        assert_eq!(restored_b.next_dbuf_index(), 0x43);

        let bytes_mut = dbuf.as_bytes_mut();
        let restored_b_mut = DataBuf::from_bytes_mut(bytes_mut);
        restored_b_mut.set_next_dbuf_index(0x44);
        assert_eq!(dbuf.next_dbuf_index(), 0x44);
    }

    #[test]
    fn ok_databuf_errors() {
        let mut dbuf = DataBuf::new();
        assert_eq!(dbuf.alloc_contiguous(0x40), None);
        let err = dbuf.write_kv_payload(0x3F, b"key", b"val");
        assert_eq!(err, Err(DataBufError::PayloadTooLarge));

        let err_read = dbuf.read_kv_payload(0x3F, 4, 8);
        assert_eq!(err_read, Err(DataBufError::InvalidIndex));

        assert_ne!(DataBufError::OutOfSpace, DataBufError::InvalidIndex);
    }
}
