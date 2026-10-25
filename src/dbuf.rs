use crate::common::{
    CRC_SIZE, DATABUF_BUF_COUNT, DATABUF_BUF_SIZE, DATABUF_FULL_BIT, DATABUF_OCCUPANCY_MASK,
    DATABUF_PAYLOAD_SIZE, DATABUF_SIZE, DATABUF_SLOTS, KV_HEADER_SIZE, LIST_KEY_HEADER_SIZE,
    LIST_VAL_HEADER_SIZE, Slot,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DataBufError {
    InvalidIndex,
    PayloadTooLarge,
    CrcMismatch { expected: u32, found: u32 },
}

/// Ref: `DESIGN.md` §1, §3.3
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DataBuf {
    buffers: [[u8; DATABUF_BUF_SIZE]; DATABUF_BUF_COUNT],
    bitmap: u64,
    next_dbuf: u32,
    padding: [u8; 4],
}

const _: () = assert!(core::mem::align_of::<DataBuf>() == 8);
const _: () = assert!(core::mem::offset_of!(DataBuf, buffers) == 0);
const _: () = assert!(core::mem::size_of::<DataBuf>() == DATABUF_SIZE);
const _: () = assert!(core::mem::offset_of!(DataBuf, bitmap) == 0x03F0);
const _: () = assert!(core::mem::offset_of!(DataBuf, padding) == 0x03FC);
const _: () = assert!(core::mem::offset_of!(DataBuf, next_dbuf) == 0x03F8);
const _: () = assert!(core::mem::size_of::<DataBuf>() == DATABUF_SLOTS * Slot::SIZE);

const _: () = assert!(CRC_SIZE == 4);
const _: () = assert!(KV_HEADER_SIZE == 4);
const _: () = assert!(DATABUF_FULL_BIT == 1);
const _: () = assert!(DATABUF_BUF_SIZE == 0x10);
const _: () = assert!(LIST_KEY_HEADER_SIZE == 5);
const _: () = assert!(LIST_VAL_HEADER_SIZE == 6);
const _: () = assert!(DATABUF_BUF_COUNT == 0x3F);
const _: () = assert!(DATABUF_PAYLOAD_SIZE == 0x03F0);
const _: () = assert!(DATABUF_SIZE % Slot::SIZE == 0);
const _: () = assert!(DATABUF_SIZE / Slot::SIZE == DATABUF_SLOTS);
const _: () = assert!(DATABUF_OCCUPANCY_MASK == 0xFFFF_FFFF_FFFF_FFFE);
const _: () = assert!(DATABUF_BUF_COUNT * DATABUF_BUF_SIZE == DATABUF_PAYLOAD_SIZE);
const _: () = assert!(
    core::mem::size_of::<[[u8; DATABUF_BUF_SIZE]; DATABUF_BUF_COUNT]>() == DATABUF_PAYLOAD_SIZE
);

const _: () = assert!(DataBuf::global_buf_idx(2, 2) == 0x80);
const _: () = assert!(DataBuf::required_kv_buffers(4, 8) == 1);
const _: () = assert!(DataBuf::required_kv_buffers(4, 9) == 2);
const _: () = assert!(DataBuf::decompose_buf_idx(0x80).1 == 2);
const _: () = assert!(DataBuf::decompose_buf_idx(0x80).0 == 2);
const _: () = assert!(DataBuf::required_list_key_buffers(0x0B) == 1);
const _: () = assert!(DataBuf::required_list_key_buffers(0x0C) == 2);
const _: () = assert!(DataBuf::required_list_val_buffers(0x0A) == 1);
const _: () = assert!(DataBuf::required_list_val_buffers(0x0B) == 2);

impl Default for DataBuf {
    #[inline(always)]
    fn default() -> Self {
        Self::new()
    }
}

impl DataBuf {
    #[inline(always)]
    pub(crate) const fn new() -> Self {
        Self {
            buffers: [[0u8; DATABUF_BUF_SIZE]; DATABUF_BUF_COUNT],
            bitmap: 0,
            next_dbuf: 0,
            padding: [0u8; 4],
        }
    }

    #[inline(always)]
    pub(crate) fn is_full(&self) -> bool {
        (self.bitmap & DATABUF_FULL_BIT) != 0
            || (self.bitmap & DATABUF_OCCUPANCY_MASK) == DATABUF_OCCUPANCY_MASK
    }

    #[inline(always)]
    pub(crate) fn is_range_free(&self, start_idx: usize, count: usize) -> bool {
        if count == 0 || start_idx + count > DATABUF_BUF_COUNT {
            return false;
        }

        let mask = if count == 0x40 { u64::MAX } else { ((1u64 << count) - 1) << (1 + start_idx) };
        (self.bitmap & mask) == 0
    }

    pub(crate) fn find_contiguous_free(&self, count: usize) -> Option<usize> {
        if count == 0 || count > DATABUF_BUF_COUNT || self.is_full() {
            return None;
        }

        let max_start = DATABUF_BUF_COUNT - count;
        (0..=max_start).find(|&idx| self.is_range_free(idx, count))
    }

    pub(crate) fn alloc_contiguous(&mut self, count: usize) -> Option<usize> {
        let start = self.find_contiguous_free(count)?;
        let mask = if count == 0x40 { u64::MAX } else { ((1u64 << count) - 1) << (1 + start) };

        self.bitmap |= mask;
        if (self.bitmap & DATABUF_OCCUPANCY_MASK) == DATABUF_OCCUPANCY_MASK {
            self.bitmap |= DATABUF_FULL_BIT;
        }

        Some(start)
    }

    pub(crate) fn free_contiguous(&mut self, start_idx: usize, count: usize) {
        // sanity check
        debug_assert!(start_idx + count <= DATABUF_BUF_COUNT);

        if count == 0 {
            return;
        }

        let mask = if count == 0x40 { u64::MAX } else { ((1u64 << count) - 1) << (1 + start_idx) };
        self.bitmap &= !mask;
        self.bitmap &= !DATABUF_FULL_BIT;
    }

    #[inline(always)]
    pub(crate) const fn required_buffers(header_size: usize, payload_len: usize) -> usize {
        (header_size + payload_len + 0x0F) / DATABUF_BUF_SIZE
    }

    /// Ref: `DESIGN.md` §3.3
    #[inline(always)]
    pub(crate) const fn required_kv_buffers(klen: usize, vlen: usize) -> usize {
        Self::required_buffers(KV_HEADER_SIZE, klen + vlen)
    }

    /// Ref: `DESIGN.md` §3.3
    #[inline(always)]
    pub(crate) const fn required_list_key_buffers(klen: usize) -> usize {
        Self::required_buffers(LIST_KEY_HEADER_SIZE, klen)
    }

    /// Ref: `DESIGN.md` §3.3
    #[inline(always)]
    pub(crate) const fn required_list_val_buffers(vlen: usize) -> usize {
        Self::required_buffers(LIST_VAL_HEADER_SIZE, vlen)
    }

    /// Ref: `DESIGN.md` §3.3
    #[inline(always)]
    pub(crate) const fn global_buf_idx(pool_idx: u32, local_idx: usize) -> u32 {
        pool_idx * (DATABUF_BUF_COUNT as u32) + (local_idx as u32)
    }

    /// Ref: `DESIGN.md` §3.3
    #[inline(always)]
    pub(crate) const fn decompose_buf_idx(global_idx: u32) -> (u32, usize) {
        let pool = global_idx / (DATABUF_BUF_COUNT as u32);
        let local = (global_idx % (DATABUF_BUF_COUNT as u32)) as usize;
        (pool, local)
    }

    #[inline(always)]
    pub(crate) fn payload_bytes(&self) -> &[u8; DATABUF_PAYLOAD_SIZE] {
        unsafe { &*(self.buffers.as_ptr() as *const [u8; DATABUF_PAYLOAD_SIZE]) }
    }

    #[inline(always)]
    pub(crate) fn payload_bytes_mut(&mut self) -> &mut [u8; DATABUF_PAYLOAD_SIZE] {
        unsafe { &mut *(self.buffers.as_mut_ptr() as *mut [u8; DATABUF_PAYLOAD_SIZE]) }
    }

    /// Ref: `DESIGN.md` §3.3
    pub(crate) fn write_kv_payload(
        &mut self,
        start_idx: usize,
        key: &[u8],
        val: &[u8],
    ) -> Result<(), DataBufError> {
        let req = Self::required_kv_buffers(key.len(), val.len());
        if req > DATABUF_BUF_COUNT || start_idx + req > DATABUF_BUF_COUNT {
            return Err(DataBufError::PayloadTooLarge);
        }

        let start_byte = start_idx * DATABUF_BUF_SIZE;
        let total_bytes = req * DATABUF_BUF_SIZE;
        let data_len = key.len() + val.len();

        let raw = self.payload_bytes_mut();
        let frame = &mut raw[start_byte..start_byte + total_bytes];

        frame[CRC_SIZE..CRC_SIZE + key.len()].copy_from_slice(key);
        frame[CRC_SIZE + key.len()..CRC_SIZE + data_len].copy_from_slice(val);
        frame[CRC_SIZE + data_len..].fill(0);

        let crc = turbo_crc::crc32c(&frame[CRC_SIZE..CRC_SIZE + data_len]);
        frame[..CRC_SIZE].copy_from_slice(&crc.to_le_bytes());

        Ok(())
    }

    /// Ref: `DESIGN.md` §3.3
    pub(crate) fn read_kv_payload(
        &self,
        start_idx: usize,
        klen: usize,
        vlen: usize,
    ) -> Result<(&[u8], &[u8]), DataBufError> {
        let req = Self::required_kv_buffers(klen, vlen);
        if req > DATABUF_BUF_COUNT || start_idx + req > DATABUF_BUF_COUNT {
            return Err(DataBufError::InvalidIndex);
        }

        let start_byte = start_idx * DATABUF_BUF_SIZE;
        let total_bytes = req * DATABUF_BUF_SIZE;
        let data_len = klen + vlen;

        let raw = self.payload_bytes();
        let frame = &raw[start_byte..start_byte + total_bytes];

        let mut crc_b = [0u8; CRC_SIZE];
        crc_b.copy_from_slice(&frame[..CRC_SIZE]);

        let expected_crc = u32::from_le_bytes(crc_b);
        let data = &frame[CRC_SIZE..CRC_SIZE + data_len];
        let actual_crc = turbo_crc::crc32c(data);

        if actual_crc != expected_crc {
            return Err(DataBufError::CrcMismatch { expected: expected_crc, found: actual_crc });
        }

        let k = &frame[CRC_SIZE..CRC_SIZE + klen];
        let v = &frame[CRC_SIZE + klen..CRC_SIZE + data_len];

        Ok((k, v))
    }

    #[inline]
    pub(crate) fn verify_key(&self, start_idx: usize, key: &[u8]) -> bool {
        let req_key = (CRC_SIZE + key.len() + 0x0F) / DATABUF_BUF_SIZE;
        if start_idx + req_key > DATABUF_BUF_COUNT {
            return false;
        }

        let start_byte = start_idx * DATABUF_BUF_SIZE + CRC_SIZE;
        let raw = self.payload_bytes();

        &raw[start_byte..start_byte + key.len()] == key
    }

    /// Ref: `DESIGN.md` §3.3
    pub(crate) fn write_list_key_payload(
        &mut self,
        start_idx: usize,
        key: &[u8],
    ) -> Result<(), DataBufError> {
        let req = Self::required_list_key_buffers(key.len());
        if req > DATABUF_BUF_COUNT || start_idx + req > DATABUF_BUF_COUNT {
            return Err(DataBufError::PayloadTooLarge);
        }

        let start_byte = start_idx * DATABUF_BUF_SIZE;
        let total_bytes = req * DATABUF_BUF_SIZE;

        let raw = self.payload_bytes_mut();
        let frame = &mut raw[start_byte..start_byte + total_bytes];

        frame[CRC_SIZE] = key.len() as u8;
        frame[LIST_KEY_HEADER_SIZE..LIST_KEY_HEADER_SIZE + key.len()].copy_from_slice(key);
        frame[LIST_KEY_HEADER_SIZE + key.len()..].fill(0);

        let crc = turbo_crc::crc32c(&frame[CRC_SIZE..LIST_KEY_HEADER_SIZE + key.len()]);
        frame[..CRC_SIZE].copy_from_slice(&crc.to_le_bytes());

        Ok(())
    }

    /// Ref: `DESIGN.md` §3.3
    #[allow(dead_code)]
    pub(crate) fn read_list_key_payload(
        &self,
        start_idx: usize,
        klen: usize,
    ) -> Result<&[u8], DataBufError> {
        let req = Self::required_list_key_buffers(klen);
        if req > DATABUF_BUF_COUNT || start_idx + req > DATABUF_BUF_COUNT {
            return Err(DataBufError::InvalidIndex);
        }

        let start_byte = start_idx * DATABUF_BUF_SIZE;
        let total_bytes = req * DATABUF_BUF_SIZE;

        let raw = self.payload_bytes();
        let frame = &raw[start_byte..start_byte + total_bytes];

        let mut crc_b = [0u8; CRC_SIZE];
        crc_b.copy_from_slice(&frame[..CRC_SIZE]);

        let expected_crc = u32::from_le_bytes(crc_b);
        let actual_crc = turbo_crc::crc32c(&frame[CRC_SIZE..LIST_KEY_HEADER_SIZE + klen]);

        if actual_crc != expected_crc {
            return Err(DataBufError::CrcMismatch { expected: expected_crc, found: actual_crc });
        }

        Ok(&frame[LIST_KEY_HEADER_SIZE..LIST_KEY_HEADER_SIZE + klen])
    }

    #[inline]
    pub(crate) fn verify_list_key(&self, start_idx: usize, key: &[u8]) -> bool {
        let req = Self::required_list_key_buffers(key.len());
        if start_idx + req > DATABUF_BUF_COUNT {
            return false;
        }

        let raw = self.payload_bytes();
        let start_byte = start_idx * DATABUF_BUF_SIZE;
        let stored_klen = raw[start_byte + CRC_SIZE] as usize;

        if stored_klen != key.len() {
            return false;
        }

        let k_start = start_byte + LIST_KEY_HEADER_SIZE;
        &raw[k_start..k_start + key.len()] == key
    }

    /// Ref: `DESIGN.md` §3.3
    pub(crate) fn write_list_val_payload(
        &mut self,
        start_idx: usize,
        val: &[u8],
    ) -> Result<(), DataBufError> {
        let req = Self::required_list_val_buffers(val.len());
        if req > DATABUF_BUF_COUNT || start_idx + req > DATABUF_BUF_COUNT {
            return Err(DataBufError::PayloadTooLarge);
        }

        let start_byte = start_idx * DATABUF_BUF_SIZE;
        let total_bytes = req * DATABUF_BUF_SIZE;

        let raw = self.payload_bytes_mut();
        let frame = &mut raw[start_byte..start_byte + total_bytes];

        let vlen_bytes = (val.len() as u16).to_le_bytes();
        frame[CRC_SIZE] = vlen_bytes[0];
        frame[CRC_SIZE + 1] = vlen_bytes[1];
        frame[LIST_VAL_HEADER_SIZE..LIST_VAL_HEADER_SIZE + val.len()].copy_from_slice(val);
        frame[LIST_VAL_HEADER_SIZE + val.len()..].fill(0);

        let crc = turbo_crc::crc32c(&frame[CRC_SIZE..LIST_VAL_HEADER_SIZE + val.len()]);
        frame[..CRC_SIZE].copy_from_slice(&crc.to_le_bytes());

        Ok(())
    }

    /// Ref: `DESIGN.md` §3.3
    pub(crate) fn read_list_val_payload(
        &self,
        start_idx: usize,
        vlen: usize,
    ) -> Result<&[u8], DataBufError> {
        let req = Self::required_list_val_buffers(vlen);
        if req > DATABUF_BUF_COUNT || start_idx + req > DATABUF_BUF_COUNT {
            return Err(DataBufError::InvalidIndex);
        }

        let start_byte = start_idx * DATABUF_BUF_SIZE;
        let total_bytes = req * DATABUF_BUF_SIZE;

        let raw = self.payload_bytes();
        let frame = &raw[start_byte..start_byte + total_bytes];

        let mut crc_b = [0u8; CRC_SIZE];
        crc_b.copy_from_slice(&frame[..CRC_SIZE]);

        let expected_crc = u32::from_le_bytes(crc_b);
        let actual_crc = turbo_crc::crc32c(&frame[CRC_SIZE..LIST_VAL_HEADER_SIZE + vlen]);

        if actual_crc != expected_crc {
            return Err(DataBufError::CrcMismatch { expected: expected_crc, found: actual_crc });
        }

        Ok(&frame[LIST_VAL_HEADER_SIZE..LIST_VAL_HEADER_SIZE + vlen])
    }

    #[inline(always)]
    pub(crate) fn next_dbuf_index(&self) -> u32 {
        self.next_dbuf
    }

    #[inline(always)]
    pub(crate) fn set_next_dbuf_index(&mut self, idx: u32) {
        self.next_dbuf = idx;
    }

    #[inline(always)]
    pub(crate) fn as_slots(&self) -> &[Slot; DATABUF_SLOTS] {
        unsafe { &*(self as *const Self as *const [Slot; DATABUF_SLOTS]) }
    }

    #[allow(dead_code)]
    #[inline(always)]
    pub(crate) fn as_slots_mut(&mut self) -> &mut [Slot; DATABUF_SLOTS] {
        unsafe { &mut *(self as *mut Self as *mut [Slot; DATABUF_SLOTS]) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_dbuf_layout_invariants() {
        assert_eq!(core::mem::size_of::<DataBuf>(), 0x0400);
        assert_eq!(core::mem::align_of::<DataBuf>(), 8);
        assert_eq!(core::mem::offset_of!(DataBuf, buffers), 0);
        assert_eq!(core::mem::offset_of!(DataBuf, bitmap), 0x03F0);
        assert_eq!(core::mem::offset_of!(DataBuf, next_dbuf), 0x03F8);
        assert_eq!(core::mem::offset_of!(DataBuf, padding), 0x03FC);
    }

    #[test]
    fn ok_dbuf_bitmap_alloc_and_free() {
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

        dbuf.free_contiguous(4, 8);
        assert!(dbuf.is_range_free(0, 0x3F));
    }

    #[test]
    fn ok_dbuf_bitmap_full_state() {
        let mut dbuf = DataBuf::new();
        assert!(!dbuf.is_full());

        let alloc_all = dbuf.alloc_contiguous(0x3F);
        assert_eq!(alloc_all, Some(0));
        assert!(dbuf.is_full());
        assert_eq!(dbuf.alloc_contiguous(1), None);

        dbuf.free_contiguous(0, 1);
        assert!(!dbuf.is_full());
    }

    #[test]
    fn ok_dbuf_sizing_and_indexing() {
        assert_eq!(DataBuf::required_kv_buffers(4, 8), 1);
        assert_eq!(DataBuf::required_kv_buffers(4, 9), 2);
        assert_eq!(DataBuf::required_list_key_buffers(0x0B), 1);
        assert_eq!(DataBuf::required_list_key_buffers(0x0C), 2);
        assert_eq!(DataBuf::required_list_val_buffers(0x0A), 1);
        assert_eq!(DataBuf::required_list_val_buffers(0x0B), 2);

        let (pool, local) = DataBuf::decompose_buf_idx(0x80);
        assert_eq!(pool, 2);
        assert_eq!(local, 2);
        assert_eq!(DataBuf::global_buf_idx(pool, local), 0x80);
    }

    #[test]
    fn ok_dbuf_kv_roundtrip() {
        let mut dbuf = DataBuf::new();
        let key = b"sample_key";
        let val = b"sample_value_payload";

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
    fn ok_dbuf_list_key_and_val_roundtrip() {
        let mut dbuf = DataBuf::new();
        let key = b"events_queue";
        let val = b"list_item_payload_data";

        let req_k = DataBuf::required_list_key_buffers(key.len());
        let start_k = dbuf.alloc_contiguous(req_k).unwrap();
        dbuf.write_list_key_payload(start_k, key).unwrap();
        assert!(dbuf.verify_list_key(start_k, key));
        assert!(!dbuf.verify_list_key(start_k, b"wrong"));

        let read_k = dbuf.read_list_key_payload(start_k, key.len()).unwrap();
        assert_eq!(read_k, key);

        let req_v = DataBuf::required_list_val_buffers(val.len());
        let start_v = dbuf.alloc_contiguous(req_v).unwrap();
        dbuf.write_list_val_payload(start_v, val).unwrap();

        let read_v = dbuf.read_list_val_payload(start_v, val.len()).unwrap();
        assert_eq!(read_v, val);
    }

    #[test]
    fn err_dbuf_crc_mismatch() {
        let mut dbuf = DataBuf::new();
        let key = b"integrity_key";
        let val = b"integrity_val";

        let req = DataBuf::required_kv_buffers(key.len(), val.len());
        let start = dbuf.alloc_contiguous(req).unwrap();
        dbuf.write_kv_payload(start, key, val).unwrap();

        let raw = dbuf.payload_bytes_mut();
        raw[start * DATABUF_BUF_SIZE + CRC_SIZE] ^= 0xFF;

        let res = dbuf.read_kv_payload(start, key.len(), val.len());
        assert!(matches!(res, Err(DataBufError::CrcMismatch { .. })));
    }

    #[test]
    fn err_dbuf_variants() {
        let mut dbuf = DataBuf::new();
        let err_too_large = dbuf.write_kv_payload(0x3F, b"k", b"v");
        assert_eq!(err_too_large, Err(DataBufError::PayloadTooLarge));

        let err_invalid_idx = dbuf.read_kv_payload(0x3F, 4, 8);
        assert_eq!(err_invalid_idx, Err(DataBufError::InvalidIndex));
    }

    #[test]
    fn ok_dbuf_as_slots() {
        let mut dbuf = DataBuf::new();
        dbuf.set_next_dbuf_index(0x42);
        assert_eq!(dbuf.next_dbuf_index(), 0x42);
        assert_eq!(dbuf.as_slots().len(), 8);
        assert_eq!(dbuf.as_slots_mut().len(), 8);
    }
}
