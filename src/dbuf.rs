use crate::common::{
    CRC_SIZE, DBUF_BUF_COUNT, DBUF_BUF_SIZE, DBUF_FULL_BIT, DBUF_OCCUPANCY_MASK, DBUF_PAYLOAD_SIZE,
    DBUF_SIZE, DBUF_SLOTS, KV_HEADER_SIZE, LIST_KEY_HEADER_SIZE, LIST_VAL_HEADER_SIZE, Slot,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DBufError {
    InvalidIndex,
    PayloadTooLarge,
    CrcMismatch { expected: u32, found: u32 },
}

/// Ref: `DESIGN.md` §1, §3.3
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DBuf {
    pub(crate) buffers: [[u8; DBUF_BUF_SIZE]; DBUF_BUF_COUNT],
    pub(crate) next_dbuf: u32,
    pub(crate) bmap: u32,
}

const _: () = assert!(core::mem::align_of::<DBuf>() == 8);
const _: () = assert!(core::mem::offset_of!(DBuf, buffers) == 0);
const _: () = assert!(core::mem::size_of::<DBuf>() == DBUF_SIZE);
const _: () = assert!(core::mem::offset_of!(DBuf, bmap) == 0xFC);
const _: () = assert!(core::mem::offset_of!(DBuf, next_dbuf) == 0xF8);
const _: () = assert!(core::mem::size_of::<DBuf>() == DBUF_SLOTS * Slot::SIZE);

const _: () = assert!(CRC_SIZE == 4);
const _: () = assert!(KV_HEADER_SIZE == 4);
const _: () = assert!(DBUF_BUF_SIZE == 8);
const _: () = assert!(LIST_KEY_HEADER_SIZE == 5);
const _: () = assert!(LIST_VAL_HEADER_SIZE == 6);
const _: () = assert!(DBUF_BUF_COUNT == 0x1F);
const _: () = assert!(DBUF_PAYLOAD_SIZE == 0xF8);
const _: () = assert!(DBUF_SIZE % Slot::SIZE == 0);
const _: () = assert!(DBUF_SIZE / Slot::SIZE == DBUF_SLOTS);
const _: () = assert!(DBUF_FULL_BIT == 0x8000_0000);
const _: () = assert!(DBUF_OCCUPANCY_MASK == 0x7FFF_FFFF);
const _: () = assert!(DBUF_BUF_COUNT * DBUF_BUF_SIZE == DBUF_PAYLOAD_SIZE);
const _: () =
    assert!(core::mem::size_of::<[[u8; DBUF_BUF_SIZE]; DBUF_BUF_COUNT]>() == DBUF_PAYLOAD_SIZE);

const _: () = assert!(DBuf::global_buf_idx(2, 2) == 0x40);
const _: () = assert!(DBuf::required_kv_buffers(4, 8) == 2);
const _: () = assert!(DBuf::required_kv_buffers(2, 2) == 1);
const _: () = assert!(DBuf::decompose_buf_idx(0x40).1 == 2);
const _: () = assert!(DBuf::decompose_buf_idx(0x40).0 == 2);
const _: () = assert!(DBuf::required_list_key_buffers(3) == 1);
const _: () = assert!(DBuf::required_list_key_buffers(4) == 2);
const _: () = assert!(DBuf::required_list_val_buffers(2) == 1);
const _: () = assert!(DBuf::required_list_val_buffers(3) == 2);

impl Default for DBuf {
    #[inline(always)]
    fn default() -> Self {
        Self::new()
    }
}

impl DBuf {
    #[inline(always)]
    pub(crate) const fn new() -> Self {
        Self { buffers: [[0u8; DBUF_BUF_SIZE]; DBUF_BUF_COUNT], next_dbuf: 0, bmap: 0 }
    }

    #[inline(always)]
    pub(crate) fn is_full(&self) -> bool {
        (self.bmap as i32) < 0 || (self.bmap & DBUF_OCCUPANCY_MASK) == DBUF_OCCUPANCY_MASK
    }

    #[inline(always)]
    pub(crate) fn is_range_free(&self, start_idx: usize, count: usize) -> bool {
        if count == 0 || start_idx + count > DBUF_BUF_COUNT {
            return false;
        }

        let mask =
            if count == 0x1F { DBUF_OCCUPANCY_MASK } else { ((1u32 << count) - 1) << start_idx };
        (self.bmap & mask) == 0
    }

    pub(crate) fn find_contiguous_free(&self, count: usize) -> Option<usize> {
        if count == 0 || count > DBUF_BUF_COUNT || self.is_full() {
            return None;
        }

        let max_start = DBUF_BUF_COUNT - count;
        (0..=max_start).find(|&idx| self.is_range_free(idx, count))
    }

    pub(crate) fn alloc_contiguous(&mut self, count: usize) -> Option<usize> {
        let start = self.find_contiguous_free(count)?;
        let mask = if count == 0x1F { DBUF_OCCUPANCY_MASK } else { ((1u32 << count) - 1) << start };

        self.bmap |= mask;
        if (self.bmap & DBUF_OCCUPANCY_MASK) == DBUF_OCCUPANCY_MASK {
            self.bmap |= DBUF_FULL_BIT;
        }

        Some(start)
    }

    pub(crate) fn free_contiguous(&mut self, start_idx: usize, count: usize) {
        debug_assert!(start_idx + count <= DBUF_BUF_COUNT);

        if count == 0 {
            return;
        }

        let mask =
            if count == 0x1F { DBUF_OCCUPANCY_MASK } else { ((1u32 << count) - 1) << start_idx };
        self.bmap &= !mask;
        self.bmap &= !DBUF_FULL_BIT;
    }

    #[inline(always)]
    pub(crate) const fn required_buffers(header_size: usize, payload_len: usize) -> usize {
        (header_size + payload_len + 7) / DBUF_BUF_SIZE
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
        pool_idx * (DBUF_BUF_COUNT as u32) + (local_idx as u32)
    }

    /// Ref: `DESIGN.md` §3.3
    #[inline(always)]
    pub(crate) const fn decompose_buf_idx(global_idx: u32) -> (u32, usize) {
        let pool = global_idx / (DBUF_BUF_COUNT as u32);
        let local = (global_idx % (DBUF_BUF_COUNT as u32)) as usize;

        (pool, local)
    }

    #[inline(always)]
    pub(crate) fn payload_bytes(&self) -> &[u8; DBUF_PAYLOAD_SIZE] {
        unsafe { &*(self.buffers.as_ptr() as *const [u8; DBUF_PAYLOAD_SIZE]) }
    }

    #[inline(always)]
    pub(crate) fn payload_bytes_mut(&mut self) -> &mut [u8; DBUF_PAYLOAD_SIZE] {
        unsafe { &mut *(self.buffers.as_mut_ptr() as *mut [u8; DBUF_PAYLOAD_SIZE]) }
    }

    /// Ref: `DESIGN.md` §3.3
    pub(crate) fn write_kv_payload(
        &mut self,
        start_idx: usize,
        key: &[u8],
        val: &[u8],
    ) -> Result<(), DBufError> {
        let req = Self::required_kv_buffers(key.len(), val.len());
        if req > DBUF_BUF_COUNT || start_idx + req > DBUF_BUF_COUNT {
            return Err(DBufError::PayloadTooLarge);
        }

        let data_len = key.len() + val.len();
        let total_bytes = req * DBUF_BUF_SIZE;
        let start_byte = start_idx * DBUF_BUF_SIZE;

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
    ) -> Result<(&[u8], &[u8]), DBufError> {
        let req = Self::required_kv_buffers(klen, vlen);
        if req > DBUF_BUF_COUNT || start_idx + req > DBUF_BUF_COUNT {
            return Err(DBufError::InvalidIndex);
        }

        let data_len = klen + vlen;
        let total_bytes = req * DBUF_BUF_SIZE;
        let start_byte = start_idx * DBUF_BUF_SIZE;

        let raw = self.payload_bytes();
        let frame = &raw[start_byte..start_byte + total_bytes];

        let mut crc_b = [0u8; CRC_SIZE];
        crc_b.copy_from_slice(&frame[..CRC_SIZE]);

        let expected_crc = u32::from_le_bytes(crc_b);
        let data = &frame[CRC_SIZE..CRC_SIZE + data_len];
        let actual_crc = turbo_crc::crc32c(data);

        if actual_crc != expected_crc {
            return Err(DBufError::CrcMismatch { expected: expected_crc, found: actual_crc });
        }

        let k = &frame[CRC_SIZE..CRC_SIZE + klen];
        let v = &frame[CRC_SIZE + klen..CRC_SIZE + data_len];

        Ok((k, v))
    }

    #[inline]
    pub(crate) fn verify_key(&self, start_idx: usize, key: &[u8]) -> bool {
        let req_key = (CRC_SIZE + key.len() + 7) / DBUF_BUF_SIZE;
        if start_idx + req_key > DBUF_BUF_COUNT {
            return false;
        }

        let start_byte = start_idx * DBUF_BUF_SIZE + CRC_SIZE;
        let raw = self.payload_bytes();

        &raw[start_byte..start_byte + key.len()] == key
    }

    /// Ref: `DESIGN.md` §3.3
    pub(crate) fn write_list_key_payload(
        &mut self,
        start_idx: usize,
        key: &[u8],
    ) -> Result<(), DBufError> {
        let req = Self::required_list_key_buffers(key.len());
        if req > DBUF_BUF_COUNT || start_idx + req > DBUF_BUF_COUNT {
            return Err(DBufError::PayloadTooLarge);
        }

        let start_byte = start_idx * DBUF_BUF_SIZE;
        let total_bytes = req * DBUF_BUF_SIZE;

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
    pub(crate) fn read_list_klen(&self, start_idx: usize) -> Result<usize, DBufError> {
        if start_idx >= DBUF_BUF_COUNT {
            return Err(DBufError::InvalidIndex);
        }

        let raw = self.payload_bytes();
        Ok(raw[start_idx * DBUF_BUF_SIZE + CRC_SIZE] as usize)
    }

    /// Ref: `DESIGN.md` §3.3
    #[allow(dead_code)]
    pub(crate) fn read_list_key_payload(
        &self,
        start_idx: usize,
        klen: usize,
    ) -> Result<&[u8], DBufError> {
        let req = Self::required_list_key_buffers(klen);
        if req > DBUF_BUF_COUNT || start_idx + req > DBUF_BUF_COUNT {
            return Err(DBufError::InvalidIndex);
        }

        let start_byte = start_idx * DBUF_BUF_SIZE;
        let total_bytes = req * DBUF_BUF_SIZE;

        let raw = self.payload_bytes();
        let frame = &raw[start_byte..start_byte + total_bytes];

        let mut crc_b = [0u8; CRC_SIZE];
        crc_b.copy_from_slice(&frame[..CRC_SIZE]);

        let expected_crc = u32::from_le_bytes(crc_b);
        let actual_crc = turbo_crc::crc32c(&frame[CRC_SIZE..LIST_KEY_HEADER_SIZE + klen]);

        if actual_crc != expected_crc {
            return Err(DBufError::CrcMismatch { expected: expected_crc, found: actual_crc });
        }

        Ok(&frame[LIST_KEY_HEADER_SIZE..LIST_KEY_HEADER_SIZE + klen])
    }

    #[inline]
    pub(crate) fn verify_list_key(&self, start_idx: usize, key: &[u8]) -> bool {
        let req = Self::required_list_key_buffers(key.len());
        if start_idx + req > DBUF_BUF_COUNT {
            return false;
        }

        let raw = self.payload_bytes();
        let start_byte = start_idx * DBUF_BUF_SIZE;
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
    ) -> Result<(), DBufError> {
        let req = Self::required_list_val_buffers(val.len());
        if req > DBUF_BUF_COUNT || start_idx + req > DBUF_BUF_COUNT {
            return Err(DBufError::PayloadTooLarge);
        }

        let start_byte = start_idx * DBUF_BUF_SIZE;
        let total_bytes = req * DBUF_BUF_SIZE;

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

    /// Inspect vlen at Bytes 4..5 of start buffer (Phase 1 of two-phase read)
    pub(crate) fn read_list_vlen(&self, start_idx: usize) -> Result<usize, DBufError> {
        if start_idx >= DBUF_BUF_COUNT {
            return Err(DBufError::InvalidIndex);
        }

        let raw = self.payload_bytes();
        let start_byte = start_idx * DBUF_BUF_SIZE;
        let vlen = u16::from_le_bytes([raw[start_byte + CRC_SIZE], raw[start_byte + CRC_SIZE + 1]])
            as usize;

        Ok(vlen)
    }

    /// Ref: `DESIGN.md` §3.3
    pub(crate) fn read_list_val_payload(
        &self,
        start_idx: usize,
        vlen: usize,
    ) -> Result<&[u8], DBufError> {
        let req = Self::required_list_val_buffers(vlen);
        if req > DBUF_BUF_COUNT || start_idx + req > DBUF_BUF_COUNT {
            return Err(DBufError::InvalidIndex);
        }

        let start_byte = start_idx * DBUF_BUF_SIZE;
        let total_bytes = req * DBUF_BUF_SIZE;

        let raw = self.payload_bytes();
        let frame = &raw[start_byte..start_byte + total_bytes];

        let mut crc_b = [0u8; CRC_SIZE];
        crc_b.copy_from_slice(&frame[..CRC_SIZE]);

        let expected_crc = u32::from_le_bytes(crc_b);
        let actual_crc = turbo_crc::crc32c(&frame[CRC_SIZE..LIST_VAL_HEADER_SIZE + vlen]);

        if actual_crc != expected_crc {
            return Err(DBufError::CrcMismatch { expected: expected_crc, found: actual_crc });
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
    pub(crate) fn as_slots(&self) -> &[Slot; DBUF_SLOTS] {
        unsafe { &*core::ptr::from_ref(self).cast() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_dbuf_layout_invariants() {
        assert_eq!(core::mem::size_of::<DBuf>(), 0x0100);
        assert_eq!(core::mem::align_of::<DBuf>(), 8);
        assert_eq!(core::mem::offset_of!(DBuf, buffers), 0);
        assert_eq!(core::mem::offset_of!(DBuf, next_dbuf), 0xF8);
        assert_eq!(core::mem::offset_of!(DBuf, bmap), 0xFC);
    }

    #[test]
    fn ok_dbuf_bitmap_alloc_and_free() {
        let mut dbuf = DBuf::new();
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
        assert!(dbuf.is_range_free(0, 0x1F));
    }

    #[test]
    fn ok_dbuf_bitmap_full_state() {
        let mut dbuf = DBuf::new();
        assert!(!dbuf.is_full());

        let alloc_all = dbuf.alloc_contiguous(0x1F);
        assert_eq!(alloc_all, Some(0));
        assert!(dbuf.is_full());
        assert_eq!(dbuf.alloc_contiguous(1), None);

        dbuf.free_contiguous(0, 1);
        assert!(!dbuf.is_full());
    }

    #[test]
    fn ok_dbuf_sizing_and_indexing() {
        assert_eq!(DBuf::required_kv_buffers(2, 2), 1);
        assert_eq!(DBuf::required_kv_buffers(4, 8), 2);
        assert_eq!(DBuf::required_list_key_buffers(3), 1);
        assert_eq!(DBuf::required_list_key_buffers(4), 2);
        assert_eq!(DBuf::required_list_val_buffers(2), 1);
        assert_eq!(DBuf::required_list_val_buffers(3), 2);

        let (pool, local) = DBuf::decompose_buf_idx(0x40);
        assert_eq!(pool, 2);
        assert_eq!(local, 2);
        assert_eq!(DBuf::global_buf_idx(pool, local), 0x40);
    }

    #[test]
    fn ok_dbuf_kv_roundtrip() {
        let mut dbuf = DBuf::new();
        let key = b"k1";
        let val = b"sample_val";

        let req = DBuf::required_kv_buffers(key.len(), val.len());
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
        let mut dbuf = DBuf::new();
        let key = b"events";
        let val = b"list_item_payload";

        let req_k = DBuf::required_list_key_buffers(key.len());
        let start_k = dbuf.alloc_contiguous(req_k).unwrap();
        dbuf.write_list_key_payload(start_k, key).unwrap();
        assert!(dbuf.verify_list_key(start_k, key));
        assert!(!dbuf.verify_list_key(start_k, b"wrong"));

        let klen = dbuf.read_list_klen(start_k).unwrap();
        assert_eq!(klen, key.len());
        let read_k = dbuf.read_list_key_payload(start_k, klen).unwrap();
        assert_eq!(read_k, key);

        let req_v = DBuf::required_list_val_buffers(val.len());
        let start_v = dbuf.alloc_contiguous(req_v).unwrap();
        dbuf.write_list_val_payload(start_v, val).unwrap();

        let vlen = dbuf.read_list_vlen(start_v).unwrap();
        assert_eq!(vlen, val.len());
        let read_v = dbuf.read_list_val_payload(start_v, vlen).unwrap();
        assert_eq!(read_v, val);
    }

    #[test]
    fn err_dbuf_crc_mismatch() {
        let mut dbuf = DBuf::new();
        let key = b"k";
        let val = b"v";

        let req = DBuf::required_kv_buffers(key.len(), val.len());
        let start = dbuf.alloc_contiguous(req).unwrap();
        dbuf.write_kv_payload(start, key, val).unwrap();

        let raw = dbuf.payload_bytes_mut();
        raw[start * DBUF_BUF_SIZE + CRC_SIZE] ^= 0xFF;

        let res = dbuf.read_kv_payload(start, key.len(), val.len());
        assert!(matches!(res, Err(DBufError::CrcMismatch { .. })));
    }

    #[test]
    fn err_dbuf_variants() {
        let mut dbuf = DBuf::new();
        let err_too_large = dbuf.write_kv_payload(0x1F, b"k", b"v");
        assert_eq!(err_too_large, Err(DBufError::PayloadTooLarge));

        let err_invalid_idx = dbuf.read_kv_payload(0x1F, 4, 8);
        assert_eq!(err_invalid_idx, Err(DBufError::InvalidIndex));
    }

    #[test]
    fn ok_dbuf_as_slots() {
        let mut dbuf = DBuf::new();
        dbuf.set_next_dbuf_index(0x42);
        assert_eq!(dbuf.next_dbuf_index(), 0x42);
        assert_eq!(dbuf.as_slots().len(), 4);
    }
}
