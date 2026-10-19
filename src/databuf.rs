//! DataBuf storage layout
//!
//! Ref: `DESIGN.md` §1, §3.5, §5.3, §7

use crate::slot::Slot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DataBufError {
    InvalidIndex,
    PayloadTooLarge,
    CrcMismatch { expected: u32, found: u32 },
}

#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DataBuf {
    pub(crate) buffers: [[u8; 0x10]; 0x3F],
    pub(crate) bitmap: u64,
    pub(crate) next_dbuf: u32,
    pub(crate) padding: [u8; 4],
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
    pub(crate) const SLOT_COUNT: usize = 8;
    pub(crate) const BUF_COUNT: usize = 0x3F;
    pub(crate) const BUF_SIZE: usize = 0x10;
    pub(crate) const PAYLOAD_BYTES: usize = 0x03F0;
    pub(crate) const CRC_SIZE: usize = 4;
    pub(crate) const FULL_BIT: u64 = 0x8000_0000_0000_0000;
    pub(crate) const OCCUPANCY_MASK: u64 = 0x7FFF_FFFF_FFFF_FFFF;
    pub(crate) const NEXT_DBUF_MASK: u32 = 0x7FFF_FFFF;

    #[inline(always)]
    pub(crate) const fn new() -> Self {
        Self { buffers: [[0u8; 0x10]; 0x3F], bitmap: 0, next_dbuf: 0, padding: [0u8; 4] }
    }

    #[inline(always)]
    pub(crate) fn is_full(&self) -> bool {
        (self.bitmap & Self::FULL_BIT) != 0
            || (self.bitmap & Self::OCCUPANCY_MASK) == Self::OCCUPANCY_MASK
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

        (0..=max_start).find(|&idx| (self.bitmap & (test_mask << idx)) == 0)
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
    pub(crate) const fn required_buffers(payload_len: usize) -> usize {
        (Self::CRC_SIZE + payload_len + 0x0F) / Self::BUF_SIZE
    }

    #[inline(always)]
    pub(crate) const fn required_kv_buffers(klen: usize, vlen: usize) -> usize {
        Self::required_buffers(klen + vlen)
    }

    #[inline(always)]
    pub(crate) const fn required_key_buffers(klen: usize) -> usize {
        Self::required_buffers(klen)
    }

    #[inline(always)]
    pub(crate) const fn required_val_buffers(vlen: usize) -> usize {
        Self::required_buffers(vlen)
    }

    #[inline(always)]
    pub(crate) const fn global_buf_idx(dbuf_idx: u32, local_idx: usize) -> u32 {
        dbuf_idx * (Self::BUF_COUNT as u32) + (local_idx as u32)
    }

    #[inline(always)]
    pub(crate) const fn decompose_buf_idx(global_idx: u32) -> (u32, usize) {
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

    pub(crate) fn read_kv_payload(
        &self,
        start_idx: usize,
        klen: usize,
        vlen: usize,
    ) -> Result<(&[u8], &[u8]), DataBufError> {
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

    pub(crate) fn read_val_payload(
        &self,
        start_idx: usize,
        vlen: usize,
    ) -> Result<&[u8], DataBufError> {
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
    pub(crate) fn next_dbuf_index(&self) -> u32 {
        self.next_dbuf & Self::NEXT_DBUF_MASK
    }

    #[inline(always)]
    pub(crate) fn set_next_dbuf_index(&mut self, idx: u32) {
        self.next_dbuf = idx & Self::NEXT_DBUF_MASK;
    }

    #[inline(always)]
    pub(crate) fn as_slots(&self) -> &[Slot; Self::SLOT_COUNT] {
        unsafe { &*(self as *const Self as *const [Slot; Self::SLOT_COUNT]) }
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
    fn ok_databuf_contiguous_alloc_and_free() {
        let mut dbuf = DataBuf::new();
        assert_eq!(dbuf.find_contiguous_free(0x10), Some(0));

        let alloc1 = dbuf.alloc_contiguous(4);
        assert_eq!(alloc1, Some(0));

        let alloc2 = dbuf.alloc_contiguous(8);
        assert_eq!(alloc2, Some(4));

        assert!(dbuf.is_range_free(0x0C, 4));
        assert!(!dbuf.is_range_free(2, 4));

        dbuf.free_contiguous(0, 4);
        assert!(dbuf.is_range_free(0, 4));
        assert!(!dbuf.is_range_free(4, 4));

        assert_eq!(dbuf.find_contiguous_free(4), Some(0));
        assert_eq!(dbuf.find_contiguous_free(0x10), Some(0x0C));

        dbuf.free_contiguous(4, 8);
        assert!(dbuf.is_range_free(0, 0x3F));
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
    fn ok_databuf_as_slots() {
        let mut dbuf = DataBuf::new();
        dbuf.set_next_dbuf_index(0x42);

        let slots = dbuf.as_slots();
        assert_eq!(slots.len(), 8);
    }

    #[test]
    fn ok_databuf_errors() {
        let mut dbuf = DataBuf::new();
        assert_eq!(dbuf.alloc_contiguous(0x40), None);
        let err = dbuf.write_kv_payload(0x3F, b"key", b"val");
        assert_eq!(err, Err(DataBufError::PayloadTooLarge));

        let err_read = dbuf.read_kv_payload(0x3F, 4, 8);
        assert_eq!(err_read, Err(DataBufError::InvalidIndex));
    }

    #[test]
    fn ok_databuf_realloc_expansion_and_shrinkage() {
        let mut dbuf = DataBuf::new();
        let idx = dbuf.alloc_contiguous(0x04).unwrap();
        assert_eq!(idx, 0);

        dbuf.free_contiguous(idx + 1, 3);
        assert!(dbuf.is_range_free(1, 3));

        let idx2 = dbuf.alloc_contiguous(3).unwrap();
        assert_eq!(idx2, 1);

        dbuf.free_contiguous(0, 0x04);
        let idx3 = dbuf.alloc_contiguous(0x08).unwrap();
        assert_eq!(idx3, 0);
    }
}
