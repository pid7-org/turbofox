use frozen_core::memmap::{MemMap, MemMapCfg, PageSize};
use std::sync::{
    Mutex,
    atomic::{AtomicU32, AtomicU64, Ordering},
};

use crate::adjarr::AdjArrayBlock;
use crate::bucket::{Bucket, ProbeResult};
use crate::cfg::{MAX_STORAGE_PAGES, MODULE_ID, PAGE_SIZE, TurboCfg};
use crate::databuf::DataBuf;
use crate::error::{TurboError, TurboResult};
use crate::hash::hash_key;
use crate::meta::Metadata;
use crate::node::Node;
use crate::slot::Slot;
use crate::ticket::TurboTicket;

const INITIAL_PAGES: u32 = 8;
const INITIAL_SLOTS: usize = 0x0100;
const MIN_FILE_LEN: usize = (INITIAL_PAGES as usize) * (PAGE_SIZE as usize);

const ROOT_NODE_SLOT: usize = (Metadata::DEFAULT_ROOT_NODE_VP as usize) * 2;
const FIRST_BUCKET_VP: u32 = 4;
const LAST_BUCKET_VP: u32 = 0x43;

const DATABUF_SLOTS: [usize; 0x0E] =
    [0x88, 0x90, 0x98, 0xA0, 0xA8, 0xB0, 0xB8, 0xC8, 0xD0, 0xD8, 0xE0, 0xE8, 0xF0, 0xF8];

pub(crate) type DbufDiff = (usize, DataBuf, DataBuf);

#[derive(Clone, Debug)]
pub(crate) struct DbufAllocPlan {
    pub(crate) start_slot: usize,
    pub(crate) local_idx: usize,
    pub(crate) touched: Vec<DbufDiff>,
}

#[derive(Debug)]
pub(crate) struct Engine {
    mmap: MemMap<Slot>,
    growth_mutex: Mutex<()>,
    total_slots: AtomicU32,
    total_entries: AtomicU64,
}

impl Engine {
    pub(crate) fn create(cfg: TurboCfg) -> TurboResult<Self> {
        let memmap_cfg = MemMapCfg {
            module_id: MODULE_ID,
            path: cfg.path,
            version: cfg.version,
            initial_t: INITIAL_SLOTS,
            sync_interval: cfg.sync_interval,
            page_size: Some(PageSize::K4),
        };

        let mmap = MemMap::<Slot>::new(memmap_cfg)?;
        let engine = Self {
            mmap,
            growth_mutex: Mutex::new(()),
            total_slots: AtomicU32::new(INITIAL_SLOTS as u32),
            total_entries: AtomicU64::new(0),
        };

        engine.format_initial_topography()?;
        engine.sync()?;

        Ok(engine)
    }

    pub(crate) fn open(cfg: TurboCfg) -> TurboResult<Self> {
        let memmap_cfg = MemMapCfg {
            module_id: MODULE_ID,
            path: cfg.path,
            version: cfg.version,
            initial_t: INITIAL_SLOTS,
            sync_interval: cfg.sync_interval,
            page_size: Some(PageSize::K4),
        };

        let mmap = MemMap::<Slot>::open(memmap_cfg)?;
        let file_len = mmap.length();

        if file_len < MIN_FILE_LEN {
            return Err(TurboError::InvalidSize { expected: MIN_FILE_LEN, found: file_len });
        }

        if file_len % (PAGE_SIZE as usize) != 0 {
            return Err(TurboError::InvalidSize {
                expected: (file_len / (PAGE_SIZE as usize)) * (PAGE_SIZE as usize),
                found: file_len,
            });
        }

        if file_len % Slot::SIZE != 0 {
            return Err(TurboError::InvalidSize {
                expected: (file_len / Slot::SIZE) * Slot::SIZE,
                found: file_len,
            });
        }

        let meta = mmap.read_exact::<Metadata, _, _>(0, |m| *m)?;
        if meta.magic != Metadata::MAGIC {
            return Err(TurboError::InvalidMagic);
        }

        if meta.version != Metadata::CURRENT_VERSION {
            return Err(TurboError::VersionMismatch {
                expected: Metadata::CURRENT_VERSION,
                found: meta.version,
            });
        }

        if meta.page_size != PAGE_SIZE {
            return Err(TurboError::InvalidPageSize(meta.page_size));
        }

        if !meta.validate() {
            return Err(TurboError::CorruptedHeader("metadata geometric invariants violated"));
        }

        let expected_bytes = (meta.total_pages as usize) * (meta.page_size as usize);
        if file_len != expected_bytes {
            return Err(TurboError::InvalidSize { expected: expected_bytes, found: file_len });
        }

        if meta.root_node_vp != Metadata::DEFAULT_ROOT_NODE_VP {
            return Err(TurboError::CorruptedHeader("unexpected root node vp index"));
        }

        let is_node = mmap.read_exact::<Node, _, _>(ROOT_NODE_SLOT, |n| n.is_node())?;
        if !is_node {
            return Err(TurboError::InvalidRootNode);
        }

        let total_slots = meta.total_slots;
        let total_entries = meta.total_entries;

        Ok(Self {
            mmap,
            growth_mutex: Mutex::new(()),
            total_slots: AtomicU32::new(total_slots),
            total_entries: AtomicU64::new(total_entries),
        })
    }

    /// Formats initial 8 physical pages into components per DESIGN.md §2
    fn format_initial_topography(&self) -> TurboResult<()> {
        let mut meta = Metadata::init(PAGE_SIZE);
        meta.adj_arr_head = 1;
        meta.databuf_head = DATABUF_SLOTS[0] as u32;

        self.mmap.write_exact::<Metadata, _, _>(0, |m| *m = meta)?;

        for slot in 1..=4 {
            let mut block = AdjArrayBlock::new();
            block.set_next_block((slot + 1) as u32);
            self.mmap.write_exact::<AdjArrayBlock, _, _>(slot, |b| *b = block)?;
        }

        let mut block5 = AdjArrayBlock::new();
        block5.set_next_block(0xC0);
        self.mmap.write_exact::<AdjArrayBlock, _, _>(5, |b| *b = block5)?;

        let root_node = Node::init_from_contiguous(FIRST_BUCKET_VP);
        self.mmap.write_exact::<Node, _, _>(ROOT_NODE_SLOT, |n| *n = root_node)?;

        let bucket = Bucket::new();
        for vp in FIRST_BUCKET_VP..=LAST_BUCKET_VP {
            let slot = (vp * 2) as usize;
            self.mmap.write_exact::<Bucket, _, _>(slot, |b| *b = bucket)?;
        }

        for i in 0..DATABUF_SLOTS.len() {
            let slot = DATABUF_SLOTS[i];
            let next = if i + 1 < DATABUF_SLOTS.len() { DATABUF_SLOTS[i + 1] as u32 } else { 0 };

            let mut dbuf = DataBuf::new();
            dbuf.set_next_dbuf_index(next);

            self.mmap.write_exact::<DataBuf, _, _>(slot, |d| *d = dbuf)?;
        }

        for slot in 0xC0..=0xC6 {
            let mut block = AdjArrayBlock::new();
            block.set_next_block((slot + 1) as u32);

            self.mmap.write_exact::<AdjArrayBlock, _, _>(slot, |b| *b = block)?;
        }

        let mut block199 = AdjArrayBlock::new();
        block199.set_next_block(0);

        self.mmap.write_exact::<AdjArrayBlock, _, _>(0xC7, |b| *b = block199)?;

        Ok(())
    }

    #[inline]
    pub(crate) fn sync(&self) -> TurboResult<()> {
        let entries = self.total_entries.load(Ordering::Acquire);
        let _ = self.mmap.write_exact::<Metadata, _, _>(0, |m| {
            m.total_entries = entries;
        });

        self.mmap.sync()?;
        Ok(())
    }

    #[inline]
    pub(crate) fn read_metadata(&self) -> TurboResult<Metadata> {
        let meta = self.mmap.read_exact::<Metadata, _, _>(0, |m| *m)?;

        Ok(meta)
    }

    #[inline]
    #[allow(unused)]
    fn total_slots(&self) -> u32 {
        self.total_slots.load(Ordering::Acquire)
    }

    #[inline]
    #[allow(unused)]
    fn growth_mutex(&self) -> &Mutex<()> {
        &self.growth_mutex
    }

    #[inline]
    #[allow(unused)]
    fn mmap(&self) -> &MemMap<Slot> {
        &self.mmap
    }

    #[inline]
    pub(crate) fn total_entries(&self) -> u64 {
        self.total_entries.load(Ordering::Acquire)
    }

    #[inline]
    pub(crate) fn total_pages(&self) -> TurboResult<u32> {
        let meta = self.read_metadata()?;
        Ok(meta.total_pages)
    }

    #[inline]
    pub(crate) fn remaining_pages(&self) -> TurboResult<u32> {
        let current = self.total_pages()?;
        Ok(MAX_STORAGE_PAGES.saturating_sub(current))
    }

    #[inline]
    pub(crate) fn remaining_bytes(&self) -> TurboResult<u64> {
        let remaining_pages = self.remaining_pages()? as u64;
        let page_size = PAGE_SIZE as u64;
        Ok(remaining_pages * page_size)
    }

    #[inline]
    fn ensure_page_capacity(&self, additional_pages: u32) -> TurboResult<()> {
        let current = self.read_metadata()?.total_pages;
        let target = current.checked_add(additional_pages).ok_or(TurboError::DatabaseFull)?;

        if target > MAX_STORAGE_PAGES {
            return Err(TurboError::DatabaseFull);
        }

        Ok(())
    }

    pub(crate) fn find_bucket(&self, hash: u32) -> TurboResult<(usize, u8)> {
        let mut curr_vp = Metadata::DEFAULT_ROOT_NODE_VP;
        let mut depth: u8 = 0;

        loop {
            let slot = (curr_vp * 2) as usize;
            let is_bucket = self.mmap.read_exact::<Bucket, _, _>(slot, |b| b.is_bucket())?;

            if is_bucket {
                return Ok((slot, depth));
            }

            let child_vp =
                self.mmap.read_exact::<Node, _, _>(slot, |n| n.route_child(hash, depth as u32))?;
            if child_vp == 0 {
                return Err(TurboError::CorruptedHeader("unmapped trie branch"));
            }

            curr_vp = child_vp;
            depth += 1;

            if depth > 5 {
                return Err(TurboError::CorruptedHeader("trie depth exceeded maximum limit"));
            }
        }
    }

    fn read_val_payload_spanned(
        &self,
        dbuf_slot: usize,
        local_idx: usize,
        klen: usize,
        vlen: usize,
    ) -> TurboResult<Vec<u8>> {
        let req = DataBuf::required_kv_buffers(klen, vlen);
        if local_idx + req <= DataBuf::BUF_COUNT {
            return self.mmap.read_exact::<DataBuf, _, _>(dbuf_slot, |dbuf| {
                let (_k, v) = dbuf.read_kv_payload(local_idx, klen, vlen)?;
                Ok(v.to_vec())
            })?;
        }

        let total_bytes = req * DataBuf::BUF_SIZE;
        let mut frame = vec![0u8; total_bytes];
        let mut dest_offset = 0;
        let mut remaining_buffers = req;
        let mut curr_slot = dbuf_slot;
        let mut is_first = true;

        while remaining_buffers > 0 {
            let curr_local = if is_first { local_idx } else { 0 };
            let count = (DataBuf::BUF_COUNT - curr_local).min(remaining_buffers);
            let src_start = curr_local * DataBuf::BUF_SIZE;
            let bytes_len = count * DataBuf::BUF_SIZE;

            self.mmap.read_exact::<DataBuf, _, _>(curr_slot, |dbuf| {
                let raw = dbuf.payload_bytes();
                frame[dest_offset..dest_offset + bytes_len]
                    .copy_from_slice(&raw[src_start..src_start + bytes_len]);
            })?;

            dest_offset += bytes_len;
            remaining_buffers -= count;
            curr_slot += DataBuf::SLOT_COUNT;
            is_first = false;
        }

        let mut crc_bytes = [0u8; DataBuf::CRC_SIZE];
        crc_bytes.copy_from_slice(&frame[..DataBuf::CRC_SIZE]);
        let expected_crc = u32::from_le_bytes(crc_bytes);

        let data_len = klen + vlen;
        let data = &frame[DataBuf::CRC_SIZE..DataBuf::CRC_SIZE + data_len];
        let actual_crc = turbo_crc::crc32c(data);

        if actual_crc != expected_crc {
            return Err(TurboError::CorruptedPayload("databuf payload crc32 mismatch"));
        }

        let val = &data[klen..klen + vlen];
        Ok(val.to_vec())
    }

    fn match_entry_key(&self, entry: &crate::bucket::BucketEntry, key: &[u8]) -> bool {
        if (entry.klen() as usize) != key.len() {
            return false;
        }

        let (dbuf_slot, local_idx) = DataBuf::decompose_buf_idx(entry.buffer_index());
        let klen = entry.klen() as usize;
        let req_key = DataBuf::required_key_buffers(klen);

        if local_idx + req_key <= DataBuf::BUF_COUNT {
            return self
                .mmap
                .read_exact::<DataBuf, _, _>(dbuf_slot as usize, |dbuf| {
                    dbuf.verify_key(local_idx, key)
                })
                .unwrap_or(false);
        }

        let mut key_buf = vec![0u8; klen];
        let mut key_read = 0;
        let mut curr_slot = dbuf_slot as usize;
        let mut is_first = true;

        while key_read < klen {
            let curr_local = if is_first { local_idx } else { 0 };
            let offset = if is_first { DataBuf::CRC_SIZE } else { 0 };
            let start_byte = curr_local * DataBuf::BUF_SIZE + offset;
            let avail = (DataBuf::BUF_SIZE * DataBuf::BUF_COUNT).saturating_sub(start_byte);
            let to_read = avail.min(klen - key_read);

            let read_res = self.mmap.read_exact::<DataBuf, _, _>(curr_slot, |dbuf| {
                let raw = dbuf.payload_bytes();
                key_buf[key_read..key_read + to_read]
                    .copy_from_slice(&raw[start_byte..start_byte + to_read]);
            });

            if read_res.is_err() {
                return false;
            }

            key_read += to_read;
            curr_slot += DataBuf::SLOT_COUNT;
            is_first = false;
        }

        key_buf == key
    }

    pub(crate) fn get(&self, key: &[u8]) -> TurboResult<Option<Vec<u8>>> {
        if key.len() > 0xFFFF {
            return Ok(None);
        }

        let hash = hash_key(key);
        let (bucket_slot, depth) = self.find_bucket(hash)?;
        let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot, |b| *b)?;
        let probe_res = bucket.probe(hash, depth, |entry| self.match_entry_key(entry, key));

        match probe_res {
            ProbeResult::Found(idx) => {
                let entry = bucket.entry(idx);
                if entry.is_list() {
                    return Err(TurboError::InvalidType("key is a list"));
                }
                let (dbuf_slot, local_idx) = DataBuf::decompose_buf_idx(entry.buffer_index());
                let val = self.read_val_payload_spanned(
                    dbuf_slot as usize,
                    local_idx,
                    entry.klen() as usize,
                    entry.vlen() as usize,
                )?;

                Ok(Some(val))
            }
            _ => Ok(None),
        }
    }

    fn check_contiguous_dbuf_fit(
        &self,
        curr: usize,
        num_dbufs: usize,
        needed: usize,
    ) -> TurboResult<(usize, bool, Vec<DbufDiff>)> {
        let total_slots = self.total_slots.load(Ordering::Acquire) as usize;
        let mut touched = Vec::with_capacity(num_dbufs);
        let first_next =
            self.mmap.read_exact::<DataBuf, _, _>(curr, |d| d.next_dbuf_index() as usize)?;

        let mut rem_buffers = needed;
        for i in 0..num_dbufs {
            let expected_slot = curr + i * DataBuf::SLOT_COUNT;
            if expected_slot + DataBuf::SLOT_COUNT > total_slots {
                return Ok((first_next, false, Vec::new()));
            }

            let dbuf = self.mmap.read_exact::<DataBuf, _, _>(expected_slot, |d| *d)?;

            if i + 1 < num_dbufs
                && dbuf.next_dbuf_index() as usize != expected_slot + DataBuf::SLOT_COUNT
            {
                return Ok((first_next, false, Vec::new()));
            }

            let count = DataBuf::BUF_COUNT.min(rem_buffers);
            if !dbuf.is_range_free(0, count) {
                return Ok((first_next, false, Vec::new()));
            }

            let mut mutated = dbuf;
            let mask = if count == 0x40 { u64::MAX } else { (1u64 << count) - 1 };
            mutated.bitmap |= mask;

            if (mutated.bitmap & DataBuf::OCCUPANCY_MASK) == DataBuf::OCCUPANCY_MASK {
                mutated.bitmap |= DataBuf::FULL_BIT;
            }

            touched.push((expected_slot, dbuf, mutated));
            rem_buffers -= count;
        }

        Ok((first_next, true, touched))
    }

    fn find_free_databuf(&self, needed: usize) -> TurboResult<Option<DbufAllocPlan>> {
        self.find_free_databuf_excluding(needed, 0)
    }

    fn find_free_databuf_excluding(
        &self,
        needed: usize,
        exclude_slot: usize,
    ) -> TurboResult<Option<DbufAllocPlan>> {
        let head = self.read_metadata()?.databuf_head as usize;
        if head == 0 {
            return Ok(None);
        }

        if needed <= DataBuf::BUF_COUNT {
            let mut curr = head;
            while curr != 0 {
                if curr == exclude_slot {
                    curr = self
                        .mmap
                        .read_exact::<DataBuf, _, _>(curr, |d| d.next_dbuf_index() as usize)?;
                    continue;
                }

                let (next, opt_alloc) = self.mmap.read_exact::<DataBuf, _, _>(curr, |d| {
                    let next = d.next_dbuf_index() as usize;
                    let mut mutated = *d;

                    match mutated.alloc_contiguous(needed) {
                        Some(local_idx) => (next, Some((local_idx, *d, mutated))),
                        None => (next, None),
                    }
                })?;

                if let Some((local_idx, orig, mutated)) = opt_alloc {
                    return Ok(Some(DbufAllocPlan {
                        start_slot: curr,
                        local_idx,
                        touched: vec![(curr, orig, mutated)],
                    }));
                }

                curr = next;
            }

            return Ok(None);
        }

        let num_dbufs = needed.div_ceil(DataBuf::BUF_COUNT);
        let mut curr = head;

        while curr != 0 {
            if curr == exclude_slot {
                curr = self
                    .mmap
                    .read_exact::<DataBuf, _, _>(curr, |d| d.next_dbuf_index() as usize)?;
                continue;
            }

            let (next, fits, touched) = self.check_contiguous_dbuf_fit(curr, num_dbufs, needed)?;
            if fits {
                return Ok(Some(DbufAllocPlan { start_slot: curr, local_idx: 0, touched }));
            }

            curr = next;
        }

        Ok(None)
    }

    fn write_kv_payload_spanned(
        touched: &mut [(usize, DataBuf, DataBuf)],
        local_idx: usize,
        req: usize,
        key: &[u8],
        val: &[u8],
    ) {
        let total_data = key.len() + val.len();
        let total_bytes = req * DataBuf::BUF_SIZE;

        let mut frame = vec![0u8; total_bytes];
        frame[DataBuf::CRC_SIZE..DataBuf::CRC_SIZE + key.len()].copy_from_slice(key);
        frame[DataBuf::CRC_SIZE + key.len()..DataBuf::CRC_SIZE + total_data].copy_from_slice(val);

        let crc = turbo_crc::crc32c(&frame[DataBuf::CRC_SIZE..DataBuf::CRC_SIZE + total_data]);
        frame[..DataBuf::CRC_SIZE].copy_from_slice(&crc.to_le_bytes());

        let mut src_off = 0;
        let mut rem_buffers = req;
        let mut is_first = true;

        for (_slot, _orig, mutated) in touched.iter_mut() {
            let curr_local = if is_first { local_idx } else { 0 };
            let count = (DataBuf::BUF_COUNT - curr_local).min(rem_buffers);
            let dst_start = curr_local * DataBuf::BUF_SIZE;
            let bytes_len = count * DataBuf::BUF_SIZE;

            mutated.payload_bytes_mut()[dst_start..dst_start + bytes_len]
                .copy_from_slice(&frame[src_off..src_off + bytes_len]);

            src_off += bytes_len;
            rem_buffers -= count;
            is_first = false;
        }
    }

    fn write_key_payload_spanned(
        touched: &mut [(usize, DataBuf, DataBuf)],
        local_idx: usize,
        req: usize,
        key: &[u8],
    ) {
        let total_bytes = req * DataBuf::BUF_SIZE;

        let mut frame = vec![0u8; total_bytes];
        frame[DataBuf::CRC_SIZE..DataBuf::CRC_SIZE + key.len()].copy_from_slice(key);

        let crc = turbo_crc::crc32c(key);
        frame[..DataBuf::CRC_SIZE].copy_from_slice(&crc.to_le_bytes());

        let mut src_off = 0;
        let mut rem_buffers = req;
        let mut is_first = true;

        for (_slot, _orig, mutated) in touched.iter_mut() {
            let curr_local = if is_first { local_idx } else { 0 };
            let count = (DataBuf::BUF_COUNT - curr_local).min(rem_buffers);
            let dst_start = curr_local * DataBuf::BUF_SIZE;
            let bytes_len = count * DataBuf::BUF_SIZE;

            mutated.payload_bytes_mut()[dst_start..dst_start + bytes_len]
                .copy_from_slice(&frame[src_off..src_off + bytes_len]);

            src_off += bytes_len;
            rem_buffers -= count;
            is_first = false;
        }
    }

    fn write_item_payload_spanned(
        touched: &mut [(usize, DataBuf, DataBuf)],
        local_idx: usize,
        req: usize,
        val: &[u8],
    ) {
        let total_bytes = req * DataBuf::BUF_SIZE;

        let mut frame = vec![0u8; total_bytes];
        frame[DataBuf::CRC_SIZE..DataBuf::CRC_SIZE + val.len()].copy_from_slice(val);

        let crc = turbo_crc::crc32c(val);
        frame[..DataBuf::CRC_SIZE].copy_from_slice(&crc.to_le_bytes());

        let mut src_off = 0;
        let mut rem_buffers = req;
        let mut is_first = true;

        for (_slot, _orig, mutated) in touched.iter_mut() {
            let curr_local = if is_first { local_idx } else { 0 };
            let count = (DataBuf::BUF_COUNT - curr_local).min(rem_buffers);
            let dst_start = curr_local * DataBuf::BUF_SIZE;
            let bytes_len = count * DataBuf::BUF_SIZE;

            mutated.payload_bytes_mut()[dst_start..dst_start + bytes_len]
                .copy_from_slice(&frame[src_off..src_off + bytes_len]);

            src_off += bytes_len;
            rem_buffers -= count;
            is_first = false;
        }
    }

    fn read_item_payload_spanned(
        &self,
        dbuf_slot: usize,
        local_idx: usize,
        vlen: usize,
    ) -> TurboResult<Vec<u8>> {
        let req = DataBuf::required_val_buffers(vlen);
        if local_idx + req <= DataBuf::BUF_COUNT {
            return self.mmap.read_exact::<DataBuf, _, _>(dbuf_slot, |dbuf| {
                let v = dbuf.read_val_payload(local_idx, vlen)?;
                Ok(v.to_vec())
            })?;
        }

        let total_bytes = req * DataBuf::BUF_SIZE;
        let mut frame = vec![0u8; total_bytes];
        let mut dest_offset = 0;
        let mut remaining_buffers = req;
        let mut curr_slot = dbuf_slot;
        let mut is_first = true;

        while remaining_buffers > 0 {
            let curr_local = if is_first { local_idx } else { 0 };
            let count = (DataBuf::BUF_COUNT - curr_local).min(remaining_buffers);
            let src_start = curr_local * DataBuf::BUF_SIZE;
            let bytes_len = count * DataBuf::BUF_SIZE;

            self.mmap.read_exact::<DataBuf, _, _>(curr_slot, |dbuf| {
                let raw = dbuf.payload_bytes();
                frame[dest_offset..dest_offset + bytes_len]
                    .copy_from_slice(&raw[src_start..src_start + bytes_len]);
            })?;

            dest_offset += bytes_len;
            remaining_buffers -= count;
            curr_slot += DataBuf::SLOT_COUNT;
            is_first = false;
        }

        let mut crc_bytes = [0u8; DataBuf::CRC_SIZE];
        crc_bytes.copy_from_slice(&frame[..DataBuf::CRC_SIZE]);
        let expected_crc = u32::from_le_bytes(crc_bytes);

        let data = &frame[DataBuf::CRC_SIZE..DataBuf::CRC_SIZE + vlen];
        let actual_crc = turbo_crc::crc32c(data);

        if actual_crc != expected_crc {
            return Err(TurboError::CorruptedPayload("databuf payload crc32 mismatch"));
        }

        Ok(data.to_vec())
    }

    fn free_spanned_databufs(
        &self,
        start_slot: usize,
        local_idx: usize,
        req: usize,
    ) -> TurboResult<Vec<(usize, DataBuf, DataBuf)>> {
        let mut freed = Vec::new();
        let mut rem_buffers = req;
        let mut curr_slot = start_slot;
        let mut is_first = true;

        while rem_buffers > 0 {
            let orig = self.mmap.read_exact::<DataBuf, _, _>(curr_slot, |d| *d)?;
            let mut mutated = orig;
            let curr_local = if is_first { local_idx } else { 0 };
            let count = (DataBuf::BUF_COUNT - curr_local).min(rem_buffers);
            mutated.free_contiguous(curr_local, count);
            freed.push((curr_slot, orig, mutated));

            rem_buffers -= count;
            curr_slot += DataBuf::SLOT_COUNT;
            is_first = false;
        }

        Ok(freed)
    }

    fn allocate_contiguous_on_touched(
        touched: &mut [(usize, DataBuf, DataBuf)],
        local_idx: usize,
        req: usize,
    ) {
        let mut rem_buffers = req;
        let mut is_first = true;

        for (_slot, _orig, mutated) in touched.iter_mut() {
            let curr_local = if is_first { local_idx } else { 0 };
            let count = (DataBuf::BUF_COUNT - curr_local).min(rem_buffers);

            let mask = if count == 0x40 { u64::MAX } else { ((1u64 << count) - 1) << curr_local };
            mutated.bitmap |= mask;

            if (mutated.bitmap & DataBuf::OCCUPANCY_MASK) == DataBuf::OCCUPANCY_MASK {
                mutated.bitmap |= DataBuf::FULL_BIT;
            }

            rem_buffers -= count;
            is_first = false;
        }
    }

    fn merge_dbuf_diffs(
        freed: Vec<(usize, DataBuf, DataBuf)>,
        allocated: Vec<(usize, DataBuf, DataBuf)>,
    ) -> Vec<(usize, DataBuf, DataBuf)> {
        let mut map: std::collections::BTreeMap<usize, (DataBuf, DataBuf)> =
            std::collections::BTreeMap::new();
        for (slot, orig, mutated) in freed {
            map.insert(slot, (orig, mutated));
        }

        for (slot, orig, mutated) in allocated {
            map.entry(slot)
                .and_modify(|(_entry_orig, entry_mut)| {
                    *entry_mut = mutated;
                })
                .or_insert((orig, mutated));
        }

        map.into_iter().map(|(slot, (orig, mutated))| (slot, orig, mutated)).collect()
    }

    /// Expands DataBuf storage pool by appending newly formatted physical pages (§5.3)
    fn grow_databufs(&self, needed: usize) -> TurboResult<()> {
        let _guard = self.growth_mutex.lock().unwrap();

        if self.find_free_databuf(needed)?.is_some() {
            return Ok(());
        }

        let pages = if needed <= 0xFC { 1 } else { needed.div_ceil(0xFC) };
        self.ensure_page_capacity(pages as u32)?;
        let added_slots = pages * 0x20;
        let old_slots = self.total_slots.load(Ordering::Acquire) as usize;

        self.mmap.grow(pages)?;
        self.total_slots.store((old_slots + added_slots) as u32, Ordering::Release);

        let num_dbufs = pages * 4;
        let old_head = self.read_metadata()?.databuf_head;

        for i in 0..num_dbufs {
            let slot = old_slots + i * 8;
            let next = if i + 1 < num_dbufs { (old_slots + (i + 1) * 8) as u32 } else { old_head };

            let mut dbuf = DataBuf::new();
            dbuf.set_next_dbuf_index(next);

            self.mmap.write_exact::<DataBuf, _, _>(slot, |d| *d = dbuf)?;
        }

        self.mmap.write_exact::<Metadata, _, _>(0, |m| {
            m.databuf_head = old_slots as u32;
            m.record_growth(pages as u32);
        })?;

        Ok(())
    }

    fn find_free_adjarr(&self) -> TurboResult<Option<(u16, usize, AdjArrayBlock, AdjArrayBlock)>> {
        let head = self.read_metadata()?.adj_arr_head as usize;
        let mut curr = head;
        let total_slots = self.total_slots.load(Ordering::Acquire) as usize;

        while curr != 0 && curr < total_slots {
            let (next, opt_alloc) = self.mmap.read_exact::<AdjArrayBlock, _, _>(curr, |b| {
                let next = b.next_block() as usize;

                if !b.is_full() {
                    let mut mutated = *b;
                    if let Some(arr_idx) = mutated.alloc_array() {
                        if mutated.is_full() {
                            mutated.set_occupied(true);
                        }

                        return (next, Some((arr_idx, *b, mutated)));
                    }
                }

                (next, None)
            })?;

            if let Some((arr_idx, orig, mutated)) = opt_alloc {
                let global_idx = crate::adjarr::global_index(curr as u32, arr_idx);
                return Ok(Some((global_idx, curr, orig, mutated)));
            }

            curr = next;
        }

        Ok(None)
    }

    /// Expands Adjacent Array block chain by allocating 1 physical OS page (§5.4)
    fn grow_adjarr(&self) -> TurboResult<()> {
        let _guard = self.growth_mutex.lock().unwrap();
        if self.find_free_adjarr()?.is_some() {
            return Ok(());
        }

        let pages = 1;
        self.ensure_page_capacity(pages as u32)?;
        let added_slots = 0x20;
        let old_slots = self.total_slots.load(Ordering::Acquire) as usize;

        self.mmap.grow(pages)?;
        self.total_slots.store((old_slots + added_slots) as u32, Ordering::Release);

        let num_blocks = 0x20;
        let old_head = self.read_metadata()?.adj_arr_head;

        for i in 0..num_blocks {
            let slot = old_slots + i;
            let next = if i + 1 < num_blocks { (old_slots + i + 1) as u32 } else { old_head };

            let mut block = AdjArrayBlock::new();
            block.set_next_block(next);

            self.mmap.write_exact::<AdjArrayBlock, _, _>(slot, |b| *b = block)?;
        }

        self.mmap.write_exact::<Metadata, _, _>(0, |m| {
            m.adj_arr_head = old_slots as u32;
            m.record_growth(pages as u32);
        })?;

        Ok(())
    }

    /// Deepens the Trie by splitting a full Bucket into a Node with 0x40 child Buckets (§5.2)
    fn split_bucket(&self, bucket_slot: usize, depth: u8) -> TurboResult<()> {
        let _guard = self.growth_mutex.lock().unwrap();

        let (is_bucket, is_full) =
            self.mmap.read_exact::<Bucket, _, _>(bucket_slot, |b| (b.is_bucket(), b.is_full()))?;
        if !is_bucket || !is_full {
            return Ok(());
        }

        self.ensure_page_capacity(4)?;
        let old_slots = self.total_slots.load(Ordering::Acquire) as usize;
        self.mmap.grow(4)?;
        let added_slots = 0x80;
        self.total_slots.store((old_slots + added_slots) as u32, Ordering::Release);

        let base_child_vp = (old_slots / 2) as u32;
        let old_bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot, |b| *b)?;

        let mut child_buckets = [Bucket::new(); 0x40];
        let next_depth = depth + 1;

        for (child_idx, entry) in old_bucket.route_entries_for_split(depth) {
            let cb = &mut child_buckets[child_idx];
            let probe_res = cb.probe(entry.hash(), next_depth, |_| false);

            match probe_res {
                ProbeResult::Vacant(vacant_slot) => {
                    *cb.entry_mut(vacant_slot) = entry;
                }
                _ => return Err(TurboError::CorruptedHeader("child bucket collision overflow")),
            }
        }

        for (i, cb) in child_buckets.iter().enumerate() {
            let child_slot = ((base_child_vp + (i as u32)) * 2) as usize;
            self.mmap.write_exact::<Bucket, _, _>(child_slot, |b| *b = *cb)?;
        }

        let mut node = Node::empty();
        for i in 0..0x40 {
            node.set_child(i, base_child_vp + (i as u32));
        }

        let mut tx = self.mmap.new_tx();
        for i in 0..2 {
            tx.write(&old_bucket.as_slots()[i], &node.as_slots()[i], bucket_slot + i)?;
        }
        tx.commit()?;

        self.mmap.write_exact::<Metadata, _, _>(0, |m| {
            m.record_growth(4);
        })?;

        Ok(())
    }

    fn occ_backoff(attempt: u32) {
        let base_us = 0x32u64.saturating_mul(1u64 << attempt.min(8)).min(0x2710);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);

        let jitter_pct = 0x4B + (nanos % 0x33);
        let wait_us = (base_us * jitter_pct) / 0x64;

        std::thread::sleep(std::time::Duration::from_micros(wait_us));
    }

    pub(crate) fn set(&self, key: &[u8], val: &[u8]) -> TurboResult<TurboTicket> {
        if key.len() > 0xFFFF || val.len() > 0xFFFF {
            return Err(TurboError::PayloadTooLarge);
        }

        let req = DataBuf::required_kv_buffers(key.len(), val.len());
        let hash = hash_key(key);

        for attempt in 0..0x20 {
            let (bucket_slot, depth) = self.find_bucket(hash)?;
            let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot, |b| *b)?;
            let probe_res = bucket.probe(hash, depth, |entry| self.match_entry_key(entry, key));

            match probe_res {
                ProbeResult::Full => {
                    self.split_bucket(bucket_slot, depth)?;
                    continue;
                }
                ProbeResult::Found(entry_idx) => {
                    let old_entry = bucket.entry(entry_idx);
                    if old_entry.is_list() {
                        return Err(TurboError::InvalidType("cannot overwrite list key with KV"));
                    }

                    let old_buf_idx = old_entry.buffer_index();
                    let (old_dbuf_slot, old_local_idx) = DataBuf::decompose_buf_idx(old_buf_idx);
                    let old_req = DataBuf::required_kv_buffers(
                        old_entry.klen() as usize,
                        old_entry.vlen() as usize,
                    );

                    let dbuf_diffs: Vec<(usize, DataBuf, DataBuf)>;
                    let new_buf_idx: u32;

                    if old_req <= DataBuf::BUF_COUNT && req <= DataBuf::BUF_COUNT {
                        if req <= old_req {
                            let orig_dbuf = self
                                .mmap
                                .read_exact::<DataBuf, _, _>(old_dbuf_slot as usize, |d| *d)?;

                            let mut mutated_dbuf = orig_dbuf;
                            if req < old_req {
                                mutated_dbuf.free_contiguous(old_local_idx + req, old_req - req);
                            }

                            if let Err(e) = mutated_dbuf.write_kv_payload(old_local_idx, key, val) {
                                return Err(TurboError::from(e));
                            }

                            dbuf_diffs = vec![(old_dbuf_slot as usize, orig_dbuf, mutated_dbuf)];
                            new_buf_idx = old_buf_idx;
                        } else {
                            let orig_old_dbuf = self
                                .mmap
                                .read_exact::<DataBuf, _, _>(old_dbuf_slot as usize, |d| *d)?;

                            let mut freed_old_dbuf = orig_old_dbuf;
                            freed_old_dbuf.free_contiguous(old_local_idx, old_req);

                            if let Some(idx) = freed_old_dbuf.alloc_contiguous(req) {
                                if let Err(e) = freed_old_dbuf.write_kv_payload(idx, key, val) {
                                    return Err(TurboError::from(e));
                                }

                                dbuf_diffs =
                                    vec![(old_dbuf_slot as usize, orig_old_dbuf, freed_old_dbuf)];
                                new_buf_idx = DataBuf::global_buf_idx(old_dbuf_slot, idx);
                            } else {
                                let mut alloc_res = match self.find_free_databuf(req)? {
                                    Some(res) => res,
                                    None => {
                                        self.grow_databufs(req)?;

                                        match self.find_free_databuf(req)? {
                                            Some(res) => res,
                                            None => {
                                                Self::occ_backoff(attempt);
                                                continue;
                                            }
                                        }
                                    }
                                };
                                Self::write_kv_payload_spanned(
                                    &mut alloc_res.touched,
                                    alloc_res.local_idx,
                                    req,
                                    key,
                                    val,
                                );

                                new_buf_idx = DataBuf::global_buf_idx(
                                    alloc_res.start_slot as u32,
                                    alloc_res.local_idx,
                                );
                                dbuf_diffs = Self::merge_dbuf_diffs(
                                    vec![(old_dbuf_slot as usize, orig_old_dbuf, freed_old_dbuf)],
                                    alloc_res.touched,
                                );
                            }
                        }
                    } else if req <= old_req {
                        let freed_old = self.free_spanned_databufs(
                            old_dbuf_slot as usize,
                            old_local_idx,
                            old_req,
                        )?;

                        let mut plan_touched = freed_old;
                        Self::allocate_contiguous_on_touched(&mut plan_touched, old_local_idx, req);
                        Self::write_kv_payload_spanned(
                            &mut plan_touched,
                            old_local_idx,
                            req,
                            key,
                            val,
                        );

                        dbuf_diffs = plan_touched;
                        new_buf_idx = old_buf_idx;
                    } else {
                        let freed_old = self.free_spanned_databufs(
                            old_dbuf_slot as usize,
                            old_local_idx,
                            old_req,
                        )?;

                        let mut alloc_res = match self.find_free_databuf(req)? {
                            Some(res) => res,
                            None => {
                                self.grow_databufs(req)?;
                                match self.find_free_databuf(req)? {
                                    Some(res) => res,
                                    None => {
                                        Self::occ_backoff(attempt);
                                        continue;
                                    }
                                }
                            }
                        };

                        Self::write_kv_payload_spanned(
                            &mut alloc_res.touched,
                            alloc_res.local_idx,
                            req,
                            key,
                            val,
                        );

                        new_buf_idx = DataBuf::global_buf_idx(
                            alloc_res.start_slot as u32,
                            alloc_res.local_idx,
                        );
                        dbuf_diffs = Self::merge_dbuf_diffs(freed_old, alloc_res.touched);
                    }

                    let mut new_bucket = bucket;
                    new_bucket.entry_mut(entry_idx).set_kv(
                        key.len() as u16,
                        val.len() as u16,
                        new_buf_idx,
                        hash,
                    );

                    let mut changes = Vec::with_capacity(2 + dbuf_diffs.len() * 8);
                    for i in 0..2 {
                        changes.push((
                            bucket_slot + i,
                            bucket.as_slots()[i],
                            new_bucket.as_slots()[i],
                        ));
                    }

                    for (slot, orig, mutated) in &dbuf_diffs {
                        for i in 0..DataBuf::SLOT_COUNT {
                            let s = slot + i;
                            let o = orig.as_slots()[i];
                            let n = mutated.as_slots()[i];
                            if o != n {
                                changes.push((s, o, n));
                            }
                        }
                    }

                    changes.sort_by_key(|(s, _, _)| *s);
                    let mut tx = self.mmap.new_tx();
                    for (s, o, n) in &changes {
                        tx.write(o, n, *s)?;
                    }

                    match tx.commit() {
                        Ok(ticket) => return Ok(TurboTicket::from(ticket)),
                        Err(_) => {
                            Self::occ_backoff(attempt);
                            continue;
                        }
                    }
                }
                ProbeResult::Vacant(entry_idx) => {
                    let mut alloc_res = match self.find_free_databuf(req)? {
                        Some(res) => res,
                        None => {
                            self.grow_databufs(req)?;
                            match self.find_free_databuf(req)? {
                                Some(res) => res,
                                None => {
                                    Self::occ_backoff(attempt);
                                    continue;
                                }
                            }
                        }
                    };

                    Self::write_kv_payload_spanned(
                        &mut alloc_res.touched,
                        alloc_res.local_idx,
                        req,
                        key,
                        val,
                    );
                    let new_buf_idx =
                        DataBuf::global_buf_idx(alloc_res.start_slot as u32, alloc_res.local_idx);
                    let dbuf_diffs = alloc_res.touched;

                    let mut new_bucket = bucket;
                    new_bucket.entry_mut(entry_idx).set_kv(
                        key.len() as u16,
                        val.len() as u16,
                        new_buf_idx,
                        hash,
                    );

                    let mut changes = Vec::with_capacity(2 + dbuf_diffs.len() * 8);
                    for i in 0..2 {
                        changes.push((
                            bucket_slot + i,
                            bucket.as_slots()[i],
                            new_bucket.as_slots()[i],
                        ));
                    }
                    for (slot, orig, mutated) in &dbuf_diffs {
                        for i in 0..DataBuf::SLOT_COUNT {
                            let s = slot + i;
                            let o = orig.as_slots()[i];
                            let n = mutated.as_slots()[i];
                            if o != n {
                                changes.push((s, o, n));
                            }
                        }
                    }

                    changes.sort_by_key(|(s, _, _)| *s);
                    let mut tx = self.mmap.new_tx();
                    for (s, o, n) in &changes {
                        tx.write(o, n, *s)?;
                    }

                    match tx.commit() {
                        Ok(ticket) => {
                            self.total_entries.fetch_add(1, Ordering::Relaxed);
                            return Ok(TurboTicket::from(ticket));
                        }
                        Err(_) => {
                            Self::occ_backoff(attempt);
                            continue;
                        }
                    }
                }
            }
        }

        Err(TurboError::ResourceBusy("transaction aborted due to OCC contention"))
    }

    pub(crate) fn del(&self, key: &[u8]) -> TurboResult<bool> {
        if key.len() > 0xFFFF {
            return Ok(false);
        }

        let hash = hash_key(key);

        for attempt in 0..0x20 {
            let (bucket_slot, depth) = self.find_bucket(hash)?;
            let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot, |b| *b)?;
            let probe_res = bucket.probe(hash, depth, |entry| self.match_entry_key(entry, key));

            match probe_res {
                ProbeResult::Found(entry_idx) => {
                    let entry = bucket.entry(entry_idx);
                    if entry.is_list() {
                        return self.list_del(key);
                    }

                    let old_buf_idx = entry.buffer_index();
                    let (old_dbuf_slot, old_local_idx) = DataBuf::decompose_buf_idx(old_buf_idx);
                    let old_req =
                        DataBuf::required_kv_buffers(entry.klen() as usize, entry.vlen() as usize);

                    let dbuf_diffs =
                        self.free_spanned_databufs(old_dbuf_slot as usize, old_local_idx, old_req)?;

                    let mut new_bucket = bucket;
                    *new_bucket.entry_mut(entry_idx) = crate::bucket::BucketEntry::tombstone();

                    let mut changes = Vec::with_capacity(2 + dbuf_diffs.len() * 8);
                    for i in 0..2 {
                        changes.push((
                            bucket_slot + i,
                            bucket.as_slots()[i],
                            new_bucket.as_slots()[i],
                        ));
                    }
                    for (slot, orig, mutated) in &dbuf_diffs {
                        for i in 0..DataBuf::SLOT_COUNT {
                            let s = slot + i;
                            let o = orig.as_slots()[i];
                            let n = mutated.as_slots()[i];
                            if o != n {
                                changes.push((s, o, n));
                            }
                        }
                    }

                    changes.sort_by_key(|(s, _, _)| *s);
                    let mut tx = self.mmap.new_tx();
                    for (s, o, n) in &changes {
                        tx.write(o, n, *s)?;
                    }

                    match tx.commit() {
                        Ok(_ticket) => {
                            self.total_entries.fetch_sub(1, Ordering::Relaxed);
                            return Ok(true);
                        }
                        Err(_) => {
                            Self::occ_backoff(attempt);
                            continue;
                        }
                    }
                }
                _ => return Ok(false),
            }
        }

        Err(TurboError::ResourceBusy("transaction aborted due to OCC contention"))
    }

    fn alloc_new_list_databufs(
        &self,
        key: &[u8],
        item: &[u8],
        req_k: usize,
        req_v: usize,
    ) -> TurboResult<(u32, u32, Vec<DbufDiff>)> {
        if req_k + req_v <= DataBuf::BUF_COUNT {
            if let Some(mut plan) = self.find_free_databuf(req_k + req_v)? {
                let k_buf_idx = DataBuf::global_buf_idx(plan.start_slot as u32, plan.local_idx);
                let v_buf_idx =
                    DataBuf::global_buf_idx(plan.start_slot as u32, plan.local_idx + req_k);
                Self::write_key_payload_spanned(&mut plan.touched, plan.local_idx, req_k, key);
                Self::write_item_payload_spanned(
                    &mut plan.touched,
                    plan.local_idx + req_k,
                    req_v,
                    item,
                );
                return Ok((k_buf_idx, v_buf_idx, plan.touched));
            }
        }

        let mut plan_k = match self.find_free_databuf(req_k)? {
            Some(p) => p,
            None => {
                self.grow_databufs(req_k)?;
                self.find_free_databuf(req_k)?
                    .ok_or(TurboError::ResourceBusy("databuf out of space"))?
            }
        };
        let k_buf_idx = DataBuf::global_buf_idx(plan_k.start_slot as u32, plan_k.local_idx);
        Self::write_key_payload_spanned(&mut plan_k.touched, plan_k.local_idx, req_k, key);

        let mut plan_v = match self.find_free_databuf_excluding(req_v, plan_k.start_slot)? {
            Some(p) => p,
            None => {
                self.grow_databufs(req_v)?;
                self.find_free_databuf_excluding(req_v, plan_k.start_slot)?
                    .ok_or(TurboError::ResourceBusy("databuf out of space"))?
            }
        };
        let v_buf_idx = DataBuf::global_buf_idx(plan_v.start_slot as u32, plan_v.local_idx);
        Self::write_item_payload_spanned(&mut plan_v.touched, plan_v.local_idx, req_v, item);

        let merged = Self::merge_dbuf_diffs(plan_k.touched, plan_v.touched);
        Ok((k_buf_idx, v_buf_idx, merged))
    }

    fn create_and_append_list(
        &self,
        bucket_slot: usize,
        bucket: Bucket,
        vacant_slot: usize,
        key: &[u8],
        hash: u32,
        item: &[u8],
    ) -> TurboResult<Option<TurboTicket>> {
        let req_k = DataBuf::required_key_buffers(key.len());
        let req_v = DataBuf::required_val_buffers(item.len());

        if self.find_free_databuf(req_k)?.is_none() {
            self.grow_databufs(req_k)?;
        }
        if self.find_free_databuf(req_v)?.is_none() {
            self.grow_databufs(req_v)?;
        }

        if self.find_free_adjarr()?.is_none() {
            self.grow_adjarr()?;
        }

        let (head_global_idx, block_slot, orig_block, mut mut_block) =
            match self.find_free_adjarr()? {
                Some(alloc) => alloc,
                None => {
                    self.grow_adjarr()?;
                    match self.find_free_adjarr()? {
                        Some(alloc) => alloc,
                        None => return Err(TurboError::ResourceBusy("adjarr out of space")),
                    }
                }
            };

        let (_, head_arr_idx) = crate::adjarr::decompose_global_index(head_global_idx);
        let (k_buf_idx, v_buf_idx, dbuf_diffs) =
            self.alloc_new_list_databufs(key, item, req_k, req_v)?;

        mut_block.array_mut(head_arr_idx).push(v_buf_idx, item.len() as u16);

        let mut new_bucket = bucket;
        new_bucket.entry_mut(vacant_slot).set_list(
            key.len() as u16,
            head_global_idx,
            k_buf_idx,
            hash,
        );

        let mut changes = Vec::with_capacity(2 + 1 + dbuf_diffs.len() * 8);
        for i in 0..2 {
            changes.push((bucket_slot + i, bucket.as_slots()[i], new_bucket.as_slots()[i]));
        }
        changes.push((block_slot, *orig_block.as_slot(), *mut_block.as_slot()));
        for (slot, orig, mutated) in &dbuf_diffs {
            for i in 0..DataBuf::SLOT_COUNT {
                let s = slot + i;
                let o = orig.as_slots()[i];
                let n = mutated.as_slots()[i];
                if o != n {
                    changes.push((s, o, n));
                }
            }
        }

        changes.sort_by_key(|(s, _, _)| *s);
        let mut tx = self.mmap.new_tx();
        for (s, o, n) in &changes {
            tx.write(o, n, *s)?;
        }

        match tx.commit() {
            Ok(ack) => {
                self.total_entries.fetch_add(1, Ordering::Relaxed);
                Ok(Some(TurboTicket::new(ack)))
            }
            Err(_) => Ok(None),
        }
    }

    fn append_to_existing_list(
        &self,
        bucket_slot: usize,
        bucket: Bucket,
        head_global_idx: u16,
        item: &[u8],
    ) -> TurboResult<Option<TurboTicket>> {
        let req_v = DataBuf::required_val_buffers(item.len());
        if self.find_free_databuf(req_v)?.is_none() {
            self.grow_databufs(req_v)?;
        }

        let mut plan_v = match self.find_free_databuf(req_v)? {
            Some(p) => p,
            None => {
                self.grow_databufs(req_v)?;
                self.find_free_databuf(req_v)?
                    .ok_or(TurboError::ResourceBusy("databuf out of space"))?
            }
        };
        let v_buf_idx = DataBuf::global_buf_idx(plan_v.start_slot as u32, plan_v.local_idx);
        Self::write_item_payload_spanned(&mut plan_v.touched, plan_v.local_idx, req_v, item);

        let mut curr_global = head_global_idx;
        loop {
            let (block_slot, array_idx) = crate::adjarr::decompose_global_index(curr_global);
            let block = self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot as usize, |b| *b)?;
            let next = block.array(array_idx).next_array();
            if next == 0 {
                break;
            }
            curr_global = next;
        }

        let (tail_block_slot, tail_array_idx) = crate::adjarr::decompose_global_index(curr_global);
        let tail_block =
            self.mmap.read_exact::<AdjArrayBlock, _, _>(tail_block_slot as usize, |b| *b)?;
        let mut new_tail_block = tail_block;

        let mut changes = Vec::new();

        if !tail_block.array(tail_array_idx).is_full() {
            new_tail_block.array_mut(tail_array_idx).push(v_buf_idx, item.len() as u16);
            changes.push((
                tail_block_slot as usize,
                *tail_block.as_slot(),
                *new_tail_block.as_slot(),
            ));
        } else {
            if self.find_free_adjarr()?.is_none() {
                self.grow_adjarr()?;
            }
            let (new_global_idx, new_block_slot, orig_new_block, mut mut_new_block) =
                match self.find_free_adjarr()? {
                    Some(alloc) => alloc,
                    None => {
                        self.grow_adjarr()?;
                        self.find_free_adjarr()?
                            .ok_or(TurboError::ResourceBusy("adjarr out of space"))?
                    }
                };
            let (_, new_array_idx) = crate::adjarr::decompose_global_index(new_global_idx);
            mut_new_block.array_mut(new_array_idx).push(v_buf_idx, item.len() as u16);

            if new_block_slot == (tail_block_slot as usize) {
                mut_new_block.array_mut(tail_array_idx).set_next_array(new_global_idx);
                changes.push((
                    tail_block_slot as usize,
                    *tail_block.as_slot(),
                    *mut_new_block.as_slot(),
                ));
            } else {
                new_tail_block.array_mut(tail_array_idx).set_next_array(new_global_idx);
                changes.push((
                    tail_block_slot as usize,
                    *tail_block.as_slot(),
                    *new_tail_block.as_slot(),
                ));
                changes.push((new_block_slot, *orig_new_block.as_slot(), *mut_new_block.as_slot()));
            }
        }

        for i in 0..2 {
            changes.push((bucket_slot + i, bucket.as_slots()[i], bucket.as_slots()[i]));
        }

        for (slot, orig, mutated) in &plan_v.touched {
            for i in 0..DataBuf::SLOT_COUNT {
                let s = slot + i;
                let o = orig.as_slots()[i];
                let n = mutated.as_slots()[i];
                if o != n {
                    changes.push((s, o, n));
                }
            }
        }

        changes.sort_by_key(|(s, _, _)| *s);
        let mut tx = self.mmap.new_tx();
        for (s, o, n) in &changes {
            tx.write(o, n, *s)?;
        }

        match tx.commit() {
            Ok(ack) => Ok(Some(TurboTicket::new(ack))),
            Err(_) => Ok(None),
        }
    }

    pub(crate) fn list_append(&self, key: &[u8], item: &[u8]) -> TurboResult<TurboTicket> {
        if key.len() > 0xFFFF || item.len() > 0xFFFF {
            return Err(TurboError::PayloadTooLarge);
        }

        let hash = hash_key(key);

        for attempt in 0..0x20 {
            let (bucket_slot, depth) = self.find_bucket(hash)?;
            let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot, |b| *b)?;
            let probe_res = bucket.probe(hash, depth, |entry| self.match_entry_key(entry, key));

            match probe_res {
                ProbeResult::Full => {
                    self.split_bucket(bucket_slot, depth)?;
                    continue;
                }
                ProbeResult::Found(entry_idx) => {
                    let entry = bucket.entry(entry_idx);
                    if !entry.is_list() {
                        return Err(TurboError::InvalidType("cannot append to KV entry"));
                    }

                    let head_global_idx = entry.vlen();
                    match self.append_to_existing_list(
                        bucket_slot,
                        bucket,
                        head_global_idx,
                        item,
                    )? {
                        Some(ticket) => return Ok(ticket),
                        None => {
                            Self::occ_backoff(attempt);
                            continue;
                        }
                    }
                }
                ProbeResult::Vacant(vacant_slot) => {
                    match self.create_and_append_list(
                        bucket_slot,
                        bucket,
                        vacant_slot,
                        key,
                        hash,
                        item,
                    )? {
                        Some(ticket) => return Ok(ticket),
                        None => {
                            Self::occ_backoff(attempt);
                            continue;
                        }
                    }
                }
            }
        }

        Err(TurboError::ResourceBusy("transaction aborted due to OCC contention"))
    }

    fn pop_from_existing_list(
        &self,
        bucket_slot: usize,
        bucket: Bucket,
        head_global_idx: u16,
    ) -> TurboResult<Option<Option<Vec<u8>>>> {
        let mut curr_global = head_global_idx;
        let mut prev_global = 0u16;
        loop {
            let (block_slot, array_idx) = crate::adjarr::decompose_global_index(curr_global);
            let block = self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot as usize, |b| *b)?;
            let next = block.array(array_idx).next_array();
            if next == 0 {
                break;
            }
            prev_global = curr_global;
            curr_global = next;
        }

        let (tail_block_slot, tail_array_idx) = crate::adjarr::decompose_global_index(curr_global);
        let tail_block =
            self.mmap.read_exact::<AdjArrayBlock, _, _>(tail_block_slot as usize, |b| *b)?;
        let mut new_tail_block = tail_block;

        let popped = new_tail_block.array_mut(tail_array_idx).pop();
        if popped.is_none() {
            return Ok(Some(None));
        }
        let item_entry = popped.unwrap();

        let (dbuf_slot, local_idx) = DataBuf::decompose_buf_idx(item_entry.buf_idx());
        let val = self.read_item_payload_spanned(
            dbuf_slot as usize,
            local_idx,
            item_entry.vlen() as usize,
        )?;

        let req_v = DataBuf::required_val_buffers(item_entry.vlen() as usize);
        let dbuf_diffs = self.free_spanned_databufs(dbuf_slot as usize, local_idx, req_v)?;

        let mut changes = Vec::new();

        if new_tail_block.array(tail_array_idx).is_empty() && prev_global != 0 {
            new_tail_block.free_array(tail_array_idx);
            new_tail_block.set_occupied(false);

            let (prev_block_slot, prev_array_idx) =
                crate::adjarr::decompose_global_index(prev_global);
            if (prev_block_slot as usize) == (tail_block_slot as usize) {
                new_tail_block.array_mut(prev_array_idx).set_next_array(0);
                changes.push((
                    tail_block_slot as usize,
                    *tail_block.as_slot(),
                    *new_tail_block.as_slot(),
                ));
            } else {
                let prev_block = self
                    .mmap
                    .read_exact::<AdjArrayBlock, _, _>(prev_block_slot as usize, |b| *b)?;
                let mut new_prev_block = prev_block;
                new_prev_block.array_mut(prev_array_idx).set_next_array(0);

                changes.push((
                    tail_block_slot as usize,
                    *tail_block.as_slot(),
                    *new_tail_block.as_slot(),
                ));
                changes.push((
                    prev_block_slot as usize,
                    *prev_block.as_slot(),
                    *new_prev_block.as_slot(),
                ));
            }
        } else {
            changes.push((
                tail_block_slot as usize,
                *tail_block.as_slot(),
                *new_tail_block.as_slot(),
            ));
        }

        for i in 0..2 {
            changes.push((bucket_slot + i, bucket.as_slots()[i], bucket.as_slots()[i]));
        }

        for (slot, orig, mutated) in &dbuf_diffs {
            for i in 0..DataBuf::SLOT_COUNT {
                let s = slot + i;
                let o = orig.as_slots()[i];
                let n = mutated.as_slots()[i];
                if o != n {
                    changes.push((s, o, n));
                }
            }
        }

        changes.sort_by_key(|(s, _, _)| *s);
        let mut tx = self.mmap.new_tx();
        for (s, o, n) in &changes {
            tx.write(o, n, *s)?;
        }

        match tx.commit() {
            Ok(_) => Ok(Some(Some(val))),
            Err(_) => Ok(None),
        }
    }

    pub(crate) fn list_pop(&self, key: &[u8]) -> TurboResult<Option<Vec<u8>>> {
        if key.len() > 0xFFFF {
            return Ok(None);
        }

        let hash = hash_key(key);

        for attempt in 0..0x20 {
            let (bucket_slot, depth) = self.find_bucket(hash)?;
            let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot, |b| *b)?;
            let probe_res = bucket.probe(hash, depth, |entry| self.match_entry_key(entry, key));

            match probe_res {
                ProbeResult::Found(entry_idx) => {
                    let entry = bucket.entry(entry_idx);
                    if !entry.is_list() {
                        return Err(TurboError::InvalidType("key is a KV entry"));
                    }

                    let head_global_idx = entry.vlen();
                    match self.pop_from_existing_list(bucket_slot, bucket, head_global_idx)? {
                        Some(opt_val) => return Ok(opt_val),
                        None => {
                            Self::occ_backoff(attempt);
                            continue;
                        }
                    }
                }
                _ => return Ok(None),
            }
        }

        Err(TurboError::ResourceBusy("transaction aborted due to OCC contention"))
    }

    pub(crate) fn list_first(&self, key: &[u8]) -> TurboResult<Option<Vec<u8>>> {
        if key.len() > 0xFFFF {
            return Ok(None);
        }

        let hash = hash_key(key);
        let (bucket_slot, depth) = self.find_bucket(hash)?;
        let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot, |b| *b)?;
        let probe_res = bucket.probe(hash, depth, |entry| self.match_entry_key(entry, key));

        match probe_res {
            ProbeResult::Found(entry_idx) => {
                let entry = bucket.entry(entry_idx);
                if !entry.is_list() {
                    return Err(TurboError::InvalidType("key is a KV entry"));
                }
                let head_global = entry.vlen();
                let (block_slot, array_idx) = crate::adjarr::decompose_global_index(head_global);
                let block =
                    self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot as usize, |b| *b)?;
                let array = block.array(array_idx);
                if let Some(first_entry) = array.entry(0) {
                    if !first_entry.is_empty() {
                        let (dbuf_slot, local_idx) =
                            DataBuf::decompose_buf_idx(first_entry.buf_idx());
                        let val = self.read_item_payload_spanned(
                            dbuf_slot as usize,
                            local_idx,
                            first_entry.vlen() as usize,
                        )?;
                        return Ok(Some(val));
                    }
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    pub(crate) fn list_last(&self, key: &[u8]) -> TurboResult<Option<Vec<u8>>> {
        if key.len() > 0xFFFF {
            return Ok(None);
        }

        let hash = hash_key(key);
        let (bucket_slot, depth) = self.find_bucket(hash)?;
        let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot, |b| *b)?;
        let probe_res = bucket.probe(hash, depth, |entry| self.match_entry_key(entry, key));

        match probe_res {
            ProbeResult::Found(entry_idx) => {
                let entry = bucket.entry(entry_idx);
                if !entry.is_list() {
                    return Err(TurboError::InvalidType("key is a KV entry"));
                }
                let mut curr_global = entry.vlen();
                let mut last_entry: Option<crate::adjarr::AdjArrayEntry> = None;
                while curr_global != 0 {
                    let (block_slot, array_idx) =
                        crate::adjarr::decompose_global_index(curr_global);
                    let block =
                        self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot as usize, |b| *b)?;
                    let array = block.array(array_idx);
                    for i in (0..crate::adjarr::AdjArrayList::ENTRY_COUNT).rev() {
                        if let Some(e) = array.entry(i) {
                            if !e.is_empty() {
                                last_entry = Some(*e);
                                break;
                            }
                        }
                    }
                    curr_global = array.next_array();
                }
                if let Some(e) = last_entry {
                    let (dbuf_slot, local_idx) = DataBuf::decompose_buf_idx(e.buf_idx());
                    let val = self.read_item_payload_spanned(
                        dbuf_slot as usize,
                        local_idx,
                        e.vlen() as usize,
                    )?;
                    return Ok(Some(val));
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    pub(crate) fn list_get_at(
        &self,
        key: &[u8],
        target_idx: usize,
    ) -> TurboResult<Option<Vec<u8>>> {
        if key.len() > 0xFFFF {
            return Ok(None);
        }

        let hash = hash_key(key);
        let (bucket_slot, depth) = self.find_bucket(hash)?;
        let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot, |b| *b)?;
        let probe_res = bucket.probe(hash, depth, |entry| self.match_entry_key(entry, key));

        match probe_res {
            ProbeResult::Found(entry_idx) => {
                let entry = bucket.entry(entry_idx);
                if !entry.is_list() {
                    return Err(TurboError::InvalidType("key is a KV entry"));
                }

                let mut curr_global = entry.vlen();
                let mut current_offset = 0;

                while curr_global != 0 {
                    let (block_slot, array_idx) =
                        crate::adjarr::decompose_global_index(curr_global);
                    let block =
                        self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot as usize, |b| *b)?;

                    let array = block.array(array_idx);

                    for i in 0..crate::adjarr::AdjArrayList::ENTRY_COUNT {
                        if let Some(e) = array.entry(i) {
                            if !e.is_empty() {
                                if current_offset == target_idx {
                                    let (dbuf_slot, local_idx) =
                                        DataBuf::decompose_buf_idx(e.buf_idx());
                                    let val = self.read_item_payload_spanned(
                                        dbuf_slot as usize,
                                        local_idx,
                                        e.vlen() as usize,
                                    )?;

                                    return Ok(Some(val));
                                }

                                current_offset += 1;
                            }
                        }
                    }

                    curr_global = array.next_array();
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    pub(crate) fn list_len(&self, key: &[u8]) -> TurboResult<usize> {
        if key.len() > 0xFFFF {
            return Ok(0);
        }

        let hash = hash_key(key);
        let (bucket_slot, depth) = self.find_bucket(hash)?;
        let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot, |b| *b)?;
        let probe_res = bucket.probe(hash, depth, |entry| self.match_entry_key(entry, key));

        match probe_res {
            ProbeResult::Found(entry_idx) => {
                let entry = bucket.entry(entry_idx);
                if !entry.is_list() {
                    return Err(TurboError::InvalidType("key is a KV entry"));
                }

                let mut count = 0;
                let mut curr_global = entry.vlen();

                while curr_global != 0 {
                    let (block_slot, array_idx) =
                        crate::adjarr::decompose_global_index(curr_global);
                    let block =
                        self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot as usize, |b| *b)?;

                    let array = block.array(array_idx);
                    count += array.len();
                    curr_global = array.next_array();
                }
                Ok(count)
            }
            _ => Ok(0),
        }
    }

    pub(crate) fn list_items(&self, key: &[u8]) -> TurboResult<Vec<Vec<u8>>> {
        if key.len() > 0xFFFF {
            return Ok(Vec::new());
        }

        let hash = hash_key(key);
        let (bucket_slot, depth) = self.find_bucket(hash)?;
        let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot, |b| *b)?;
        let probe_res = bucket.probe(hash, depth, |entry| self.match_entry_key(entry, key));

        match probe_res {
            ProbeResult::Found(entry_idx) => {
                let entry = bucket.entry(entry_idx);
                if !entry.is_list() {
                    return Err(TurboError::InvalidType("key is a KV entry"));
                }
                let mut items = Vec::new();
                let mut curr_global = entry.vlen();
                while curr_global != 0 {
                    let (block_slot, array_idx) =
                        crate::adjarr::decompose_global_index(curr_global);
                    let block =
                        self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot as usize, |b| *b)?;
                    let array = block.array(array_idx);
                    for i in 0..crate::adjarr::AdjArrayList::ENTRY_COUNT {
                        if let Some(e) = array.entry(i) {
                            if !e.is_empty() {
                                let (dbuf_slot, local_idx) =
                                    DataBuf::decompose_buf_idx(e.buf_idx());
                                let val = self.read_item_payload_spanned(
                                    dbuf_slot as usize,
                                    local_idx,
                                    e.vlen() as usize,
                                )?;
                                items.push(val);
                            }
                        }
                    }
                    curr_global = array.next_array();
                }
                Ok(items)
            }
            _ => Ok(Vec::new()),
        }
    }

    pub(crate) fn list_del(&self, key: &[u8]) -> TurboResult<bool> {
        if key.len() > 0xFFFF {
            return Ok(false);
        }

        let hash = hash_key(key);

        for attempt in 0..0x20 {
            let (bucket_slot, depth) = self.find_bucket(hash)?;
            let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot, |b| *b)?;
            let probe_res = bucket.probe(hash, depth, |entry| self.match_entry_key(entry, key));

            match probe_res {
                ProbeResult::Found(entry_idx) => {
                    let entry = bucket.entry(entry_idx);
                    if !entry.is_list() {
                        return Err(TurboError::InvalidType("key is a KV entry"));
                    }

                    let head_global_idx = entry.vlen();
                    let (k_slot, k_local) = DataBuf::decompose_buf_idx(entry.buffer_index());
                    let req_k = DataBuf::required_key_buffers(entry.klen() as usize);
                    let mut dbuf_diffs =
                        self.free_spanned_databufs(k_slot as usize, k_local, req_k)?;

                    use std::collections::BTreeMap;
                    let mut block_map: BTreeMap<usize, (AdjArrayBlock, AdjArrayBlock)> =
                        BTreeMap::new();

                    let mut curr_global = head_global_idx;
                    while curr_global != 0 {
                        let (block_slot, array_idx) =
                            crate::adjarr::decompose_global_index(curr_global);
                        let slot_usize = block_slot as usize;

                        let orig_block = if let Some((o, _)) = block_map.get(&slot_usize) {
                            *o
                        } else {
                            self.mmap.read_exact::<AdjArrayBlock, _, _>(slot_usize, |b| *b)?
                        };

                        let mut mut_block = if let Some((_, m)) = block_map.get(&slot_usize) {
                            *m
                        } else {
                            orig_block
                        };

                        let next_global = mut_block.array(array_idx).next_array();

                        for entry_idx in 0..crate::adjarr::AdjArrayList::ENTRY_COUNT {
                            if let Some(item_entry) = mut_block.array(array_idx).entry(entry_idx) {
                                if !item_entry.is_empty() {
                                    let (v_slot, v_local) =
                                        DataBuf::decompose_buf_idx(item_entry.buf_idx());
                                    let req_v =
                                        DataBuf::required_val_buffers(item_entry.vlen() as usize);
                                    let diffs = self.free_spanned_databufs(
                                        v_slot as usize,
                                        v_local,
                                        req_v,
                                    )?;
                                    dbuf_diffs = Self::merge_dbuf_diffs(dbuf_diffs, diffs);
                                }
                            }
                        }

                        mut_block.free_array(array_idx);
                        mut_block.set_occupied(false);
                        block_map.insert(slot_usize, (orig_block, mut_block));

                        curr_global = next_global;
                    }

                    let mut new_bucket = bucket;
                    new_bucket.entry_mut(entry_idx).mark_tombstone();

                    let mut changes =
                        Vec::with_capacity(2 + block_map.len() + dbuf_diffs.len() * 8);
                    for i in 0..2 {
                        changes.push((
                            bucket_slot + i,
                            bucket.as_slots()[i],
                            new_bucket.as_slots()[i],
                        ));
                    }
                    for (slot, (orig, mutated)) in &block_map {
                        changes.push((*slot, *orig.as_slot(), *mutated.as_slot()));
                    }
                    for (slot, orig, mutated) in &dbuf_diffs {
                        for i in 0..DataBuf::SLOT_COUNT {
                            let s = slot + i;
                            let o = orig.as_slots()[i];
                            let n = mutated.as_slots()[i];
                            if o != n {
                                changes.push((s, o, n));
                            }
                        }
                    }

                    changes.sort_by_key(|(s, _, _)| *s);
                    let mut tx = self.mmap.new_tx();
                    for (s, o, n) in &changes {
                        tx.write(o, n, *s)?;
                    }

                    match tx.commit() {
                        Ok(_) => {
                            self.total_entries.fetch_sub(1, Ordering::Relaxed);
                            return Ok(true);
                        }
                        Err(_) => {
                            Self::occ_backoff(attempt);
                            continue;
                        }
                    }
                }
                _ => return Ok(false),
            }
        }

        Err(TurboError::ResourceBusy("transaction aborted due to OCC contention"))
    }

    pub(crate) fn delete(self) -> TurboResult<()> {
        self.mmap.delete()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_versioned_path(base: &std::path::Path, version: u32) -> std::path::PathBuf {
        let stem = base.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let ext = base.extension().and_then(|e| e.to_str());
        let filename = match ext {
            Some(e) if !e.is_empty() => format!("{}_{}.{}", stem, version, e),
            _ => format!("{}_{}", stem, version),
        };
        match base.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.join(filename),
            _ => std::path::PathBuf::from(filename),
        }
    }

    struct TempFile {
        path: std::path::PathBuf,
        versioned: std::path::PathBuf,
    }

    impl TempFile {
        fn new(name: &str) -> Self {
            let mut path = std::env::temp_dir();
            let pid = std::process::id();
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            path.push(format!("tf_{}_{}_{}.db", name, pid, ts));
            let versioned = test_versioned_path(&path, Metadata::CURRENT_VERSION);
            if versioned.exists() {
                let _ = std::fs::remove_file(&versioned);
            }
            Self { path, versioned }
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            if self.versioned.exists() {
                let _ = std::fs::remove_file(&self.versioned);
            }
        }
    }

    #[test]
    fn ok_engine_create_and_verify_topography() {
        let temp = TempFile::new("ok_topography");
        let cfg = TurboCfg::new(&temp.path);
        let engine = Engine::create(cfg.clone()).unwrap();

        assert_eq!(engine.total_slots(), INITIAL_SLOTS as u32);
        assert!(temp.versioned.exists());

        let meta = std::fs::metadata(&temp.versioned).unwrap();
        assert_eq!(meta.len() as usize, MIN_FILE_LEN);

        let header = engine.read_metadata().unwrap();
        assert_eq!(header.magic, Metadata::MAGIC);
        assert_eq!(header.version, Metadata::CURRENT_VERSION);
        assert_eq!(header.page_size, PAGE_SIZE);
        assert_eq!(header.total_pages, INITIAL_PAGES);
        assert_eq!(header.total_slots, INITIAL_SLOTS as u32);
        assert_eq!(header.total_entries, 0);
        assert_eq!(header.adj_arr_head, 1);
        assert_eq!(header.databuf_head, 0x88);

        let root_node = engine.mmap().read_exact::<Node, _, _>(ROOT_NODE_SLOT, |n| *n).unwrap();
        assert!(root_node.is_node());
        for i in 0..0x40 {
            assert_eq!(root_node.get_child(i), (FIRST_BUCKET_VP + (i as u32)));
        }

        let first_bucket = engine
            .mmap()
            .read_exact::<Bucket, _, _>((FIRST_BUCKET_VP * 2) as usize, |b| *b)
            .unwrap();
        assert!(first_bucket.is_bucket());

        let last_bucket = engine
            .mmap()
            .read_exact::<Bucket, _, _>((LAST_BUCKET_VP * 2) as usize, |b| *b)
            .unwrap();
        assert!(last_bucket.is_bucket());

        let first_dbuf =
            engine.mmap().read_exact::<DataBuf, _, _>(DATABUF_SLOTS[0], |d| *d).unwrap();
        assert_eq!(first_dbuf.next_dbuf_index(), DATABUF_SLOTS[1] as u32);

        let last_dbuf =
            engine.mmap().read_exact::<DataBuf, _, _>(DATABUF_SLOTS[0x0D], |d| *d).unwrap();
        assert_eq!(last_dbuf.next_dbuf_index(), 0);

        let block1 = engine.mmap().read_exact::<AdjArrayBlock, _, _>(1, |b| *b).unwrap();
        assert_eq!(block1.next_block(), 2);

        let block5 = engine.mmap().read_exact::<AdjArrayBlock, _, _>(5, |b| *b).unwrap();
        assert_eq!(block5.next_block(), 0xC0);

        let block199 = engine.mmap().read_exact::<AdjArrayBlock, _, _>(0xC7, |b| *b).unwrap();
        assert_eq!(block199.next_block(), 0);
    }

    #[test]
    fn ok_engine_reopen_existing_valid_file() {
        let temp = TempFile::new("ok_reopen");
        let cfg = TurboCfg::new(&temp.path);

        {
            let engine = Engine::create(cfg.clone()).unwrap();
            assert_eq!(engine.total_slots(), INITIAL_SLOTS as u32);
        }

        let reopened = Engine::open(cfg).unwrap();
        assert_eq!(reopened.total_slots(), INITIAL_SLOTS as u32);
        let header = reopened.read_metadata().unwrap();
        assert_eq!(header.magic, Metadata::MAGIC);
    }

    #[test]
    fn err_engine_create_already_exists() {
        let temp = TempFile::new("err_create_exists");
        let cfg = TurboCfg::new(&temp.path);

        let _engine = Engine::create(cfg.clone()).unwrap();
        let err = Engine::create(cfg).unwrap_err();
        match err {
            TurboError::Storage { .. } => {}
            other => panic!("unexpected error variant: {:?}", other),
        }
    }

    #[test]
    fn err_engine_open_not_exists() {
        let temp = TempFile::new("err_open_not_exists");
        let cfg = TurboCfg::new(&temp.path);

        let err = Engine::open(cfg).unwrap_err();
        match err {
            TurboError::Storage { .. } => {}
            other => panic!("unexpected error variant: {:?}", other),
        }
    }

    #[test]
    fn ok_engine_multiple_version_instances() {
        let temp1 = TempFile::new("ok_ver1");
        let path = temp1.path.clone();
        let cfg1 = TurboCfg::new(&path).with_version(1);
        let cfg2 = TurboCfg::new(&path).with_version(2);

        let engine1 = Engine::create(cfg1.clone()).unwrap();
        let engine2 = Engine::create(cfg2.clone()).unwrap();

        assert_eq!(engine1.total_slots(), INITIAL_SLOTS as u32);
        assert_eq!(engine2.total_slots(), INITIAL_SLOTS as u32);

        let ver1_path = test_versioned_path(&path, 1);
        let ver2_path = test_versioned_path(&path, 2);
        assert!(ver1_path.exists());
        assert!(ver2_path.exists());

        let _ = std::fs::remove_file(&ver1_path);
        let _ = std::fs::remove_file(&ver2_path);
    }

    #[test]
    fn err_engine_open_validation_size() {
        let temp = TempFile::new("err_size");
        std::fs::write(&temp.versioned, [0u8; 0x1000]).unwrap();

        let cfg = TurboCfg::new(&temp.path);
        let err = Engine::open(cfg).unwrap_err();
        match err {
            TurboError::InvalidSize { expected, found } => {
                assert_eq!(expected, MIN_FILE_LEN);
                assert_eq!(found, 0x1000);
            }
            TurboError::Storage { .. } => {}
            other => panic!("unexpected error: {:?}", other),
        }
    }

    #[test]
    fn err_engine_open_validation_magic() {
        let temp = TempFile::new("err_magic");
        let cfg = TurboCfg::new(&temp.path);

        {
            let engine = Engine::create(cfg.clone()).unwrap();
            let mut bad_meta = engine.read_metadata().unwrap();
            bad_meta.magic = *b"CORR";
            engine.mmap().write_exact::<Metadata, _, _>(0, |m| *m = bad_meta).unwrap();
            engine.sync().unwrap();
        }

        let err = Engine::open(cfg).unwrap_err();
        match err {
            TurboError::InvalidMagic => {}
            other => panic!("unexpected error: {:?}", other),
        }
    }

    #[test]
    fn err_engine_open_validation_version() {
        let temp = TempFile::new("err_version");
        let cfg = TurboCfg::new(&temp.path);

        {
            let engine = Engine::create(cfg.clone()).unwrap();
            let mut bad_meta = engine.read_metadata().unwrap();
            bad_meta.version = 0x02;
            engine.mmap().write_exact::<Metadata, _, _>(0, |m| *m = bad_meta).unwrap();
            engine.sync().unwrap();
        }

        let err = Engine::open(cfg).unwrap_err();
        match err {
            TurboError::VersionMismatch { expected, found } => {
                assert_eq!(expected, Metadata::CURRENT_VERSION);
                assert_eq!(found, 0x02);
            }
            other => panic!("unexpected error: {:?}", other),
        }
    }

    #[test]
    fn err_engine_open_validation_root_node() {
        let temp = TempFile::new("err_root_node");
        let cfg = TurboCfg::new(&temp.path);

        {
            let engine = Engine::create(cfg.clone()).unwrap();
            let bucket_disguised = Bucket::new();
            engine
                .mmap()
                .write_exact::<Bucket, _, _>(ROOT_NODE_SLOT, |b| *b = bucket_disguised)
                .unwrap();
            engine.sync().unwrap();
        }

        let err = Engine::open(cfg).unwrap_err();
        match err {
            TurboError::InvalidRootNode => {}
            other => panic!("unexpected error: {:?}", other),
        }
    }

    #[test]
    fn ok_engine_set_update_shrink_frees_buffers() {
        let temp = TempFile::new("ok_engine_shrink");
        let cfg = TurboCfg::new(&temp.path);
        let engine = Engine::create(cfg).unwrap();

        let initial_val = vec![0x11; 0x60];
        engine.set(b"shrink_key", &initial_val).unwrap();
        assert_eq!(engine.get(b"shrink_key").unwrap(), Some(initial_val));
        assert_eq!(engine.total_entries(), 1);

        let tiny_val = b"tiny";
        engine.set(b"shrink_key", tiny_val).unwrap();
        assert_eq!(engine.get(b"shrink_key").unwrap(), Some(tiny_val.to_vec()));
        assert_eq!(engine.total_entries(), 1);

        let _ = std::fs::remove_file(&temp.versioned);
    }

    #[test]
    fn ok_engine_update_expansion_triggers_databuf_growth() {
        let temp = TempFile::new("ok_engine_expansion_growth");
        let cfg = TurboCfg::new(&temp.path);
        let engine = Engine::create(cfg).unwrap();

        let initial_slots = engine.total_slots();

        engine.set(b"target_expand", b"small").unwrap();

        for i in 0..0x64 {
            let k = format!("fill_key_{:04x}", i);
            let v = vec![(i & 0xFF) as u8; 0x70];
            engine.set(k.as_bytes(), &v).unwrap();
        }

        let large_val = vec![0x7A; 0x0180];
        engine.set(b"target_expand", &large_val).unwrap();

        assert!(engine.total_slots() > initial_slots);
        assert_eq!(engine.get(b"target_expand").unwrap(), Some(large_val));

        let _ = std::fs::remove_file(&temp.versioned);
    }

    #[test]
    fn err_engine_set_payload_limits() {
        let temp = TempFile::new("err_engine_payload_limits");
        let cfg = TurboCfg::new(&temp.path);
        let engine = Engine::create(cfg).unwrap();

        let oversized_val = vec![0xEE; 0x0001_0000];
        let err = engine.set(b"oversized", &oversized_val).unwrap_err();
        match err {
            TurboError::PayloadTooLarge => {}
            other => panic!("unexpected error variant: {:?}", other),
        }

        let _ = std::fs::remove_file(&temp.versioned);
    }

    #[test]
    fn ok_engine_set_spanned_multibuf_roundtrip() {
        let temp = TempFile::new("ok_engine_spanned_multibuf");
        let cfg = TurboCfg::new(&temp.path);
        let engine = Engine::create(cfg).unwrap();

        let large_val = vec![0xAB; 0x0800];
        engine.set(b"large_key", &large_val).unwrap();
        assert_eq!(engine.get(b"large_key").unwrap(), Some(large_val));

        let huge_val = vec![0xCD; 0x1200];
        engine.set(b"large_key", &huge_val).unwrap();
        assert_eq!(engine.get(b"large_key").unwrap(), Some(huge_val));

        let small_val = vec![0xEF; 0x0200];
        engine.set(b"large_key", &small_val).unwrap();
        assert_eq!(engine.get(b"large_key").unwrap(), Some(small_val));

        let _ = std::fs::remove_file(&temp.versioned);
    }

    #[test]
    fn ok_engine_explicit_sync_flushes_metadata() {
        let temp = TempFile::new("ok_engine_sync_metadata");
        let cfg = TurboCfg::new(&temp.path);
        let engine = Engine::create(cfg.clone()).unwrap();

        engine.set(b"meta_sync_k1", b"v1").unwrap();
        engine.set(b"meta_sync_k2", b"v2").unwrap();
        assert_eq!(engine.total_entries(), 2);

        engine.sync().unwrap();
        drop(engine);

        let reopened = Engine::open(cfg).unwrap();
        assert_eq!(reopened.total_entries(), 2);
        let header = reopened.read_metadata().unwrap();
        assert_eq!(header.total_entries, 2);
        assert_eq!(reopened.get(b"meta_sync_k1").unwrap(), Some(b"v1".to_vec()));
        assert_eq!(reopened.get(b"meta_sync_k2").unwrap(), Some(b"v2".to_vec()));

        let _ = std::fs::remove_file(&temp.versioned);
    }

    #[test]
    fn ok_engine_drop_and_reopen_invariants() {
        let temp = TempFile::new("ok_engine_drop_invariants");
        let cfg = TurboCfg::new(&temp.path);

        {
            let engine = Engine::create(cfg.clone()).unwrap();
            for i in 0..0x08 {
                let k = format!("drop_k_{:02x}", i);
                let v = format!("drop_v_{:02x}", i);
                engine.set(k.as_bytes(), v.as_bytes()).unwrap();
            }
            engine.sync().unwrap();
        }

        let reopened = Engine::open(cfg).unwrap();
        assert_eq!(reopened.total_entries(), 0x08);
        for i in 0..0x08 {
            let k = format!("drop_k_{:02x}", i);
            let v = format!("drop_v_{:02x}", i);
            assert_eq!(reopened.get(k.as_bytes()).unwrap(), Some(v.into_bytes()));
        }

        let _ = std::fs::remove_file(&temp.versioned);
    }

    #[test]
    fn ok_engine_del_non_existent() {
        let temp = TempFile::new("ok_engine_del_non_existent");
        let cfg = TurboCfg::new(&temp.path);
        let engine = Engine::create(cfg).unwrap();

        assert!(!engine.del(b"missing_key").unwrap());
        assert_eq!(engine.total_entries(), 0);

        let _ = std::fs::remove_file(&temp.versioned);
    }

    #[test]
    fn ok_engine_del_existing_roundtrip() {
        let temp = TempFile::new("ok_engine_del_existing");
        let cfg = TurboCfg::new(&temp.path);
        let engine = Engine::create(cfg).unwrap();

        engine.set(b"del_key_1", b"val_1").unwrap();
        engine.set(b"del_key_2", b"val_2").unwrap();
        assert_eq!(engine.total_entries(), 2);

        assert!(engine.del(b"del_key_1").unwrap());
        assert_eq!(engine.total_entries(), 1);
        assert_eq!(engine.get(b"del_key_1").unwrap(), None);
        assert_eq!(engine.get(b"del_key_2").unwrap(), Some(b"val_2".to_vec()));

        assert!(!engine.del(b"del_key_1").unwrap());
        assert_eq!(engine.total_entries(), 1);

        assert!(engine.del(b"del_key_2").unwrap());
        assert_eq!(engine.total_entries(), 0);
        assert_eq!(engine.get(b"del_key_2").unwrap(), None);

        let _ = std::fs::remove_file(&temp.versioned);
    }

    #[test]
    fn ok_engine_del_spanned_multibuf() {
        let temp = TempFile::new("ok_engine_del_spanned");
        let cfg = TurboCfg::new(&temp.path);
        let engine = Engine::create(cfg).unwrap();

        let large_val = vec![0x5A; 0x0800];
        engine.set(b"spanned_del_key", &large_val).unwrap();
        assert_eq!(engine.total_entries(), 1);
        assert_eq!(engine.get(b"spanned_del_key").unwrap(), Some(large_val));

        assert!(engine.del(b"spanned_del_key").unwrap());
        assert_eq!(engine.total_entries(), 0);
        assert_eq!(engine.get(b"spanned_del_key").unwrap(), None);

        let _ = std::fs::remove_file(&temp.versioned);
    }

    #[test]
    fn ok_engine_list_grow_and_reduction_reclaims_adjarr_block() {
        let temp = TempFile::new("ok_engine_list_recycle");
        let cfg = TurboCfg::new(&temp.path);
        let engine = Engine::create(cfg).unwrap();

        for i in 0..9 {
            let item = format!("item_{}", i);
            engine.list_append(b"recycle_key", item.as_bytes()).unwrap();
        }

        assert_eq!(engine.list_len(b"recycle_key").unwrap(), 9);

        let block1 = engine.mmap.read_exact::<AdjArrayBlock, _, _>(1, |b| *b).unwrap();
        assert_eq!(block1.occupied_count(), 3);
        assert!(block1.is_array_occupied(0));
        assert!(block1.is_array_occupied(1));
        assert!(block1.is_array_occupied(2));
        assert!(!block1.is_array_occupied(3));

        for _ in 0..3 {
            assert!(engine.list_pop(b"recycle_key").unwrap().is_some());
        }
        assert_eq!(engine.list_len(b"recycle_key").unwrap(), 6);

        let block1_after_pop1 = engine.mmap.read_exact::<AdjArrayBlock, _, _>(1, |b| *b).unwrap();
        assert_eq!(block1_after_pop1.occupied_count(), 2);
        assert!(block1_after_pop1.is_array_occupied(0));
        assert!(block1_after_pop1.is_array_occupied(1));
        assert!(!block1_after_pop1.is_array_occupied(2));

        for _ in 0..3 {
            assert!(engine.list_pop(b"recycle_key").unwrap().is_some());
        }
        assert_eq!(engine.list_len(b"recycle_key").unwrap(), 3);

        let block1_after_pop2 = engine.mmap.read_exact::<AdjArrayBlock, _, _>(1, |b| *b).unwrap();
        assert_eq!(block1_after_pop2.occupied_count(), 1);
        assert!(block1_after_pop2.is_array_occupied(0));
        assert!(!block1_after_pop2.is_array_occupied(1));
        assert!(!block1_after_pop2.is_array_occupied(2));

        for _ in 0..3 {
            assert!(engine.list_pop(b"recycle_key").unwrap().is_some());
        }
        assert_eq!(engine.list_len(b"recycle_key").unwrap(), 0);
        assert_eq!(engine.list_pop(b"recycle_key").unwrap(), None);

        let block1_empty_list = engine.mmap.read_exact::<AdjArrayBlock, _, _>(1, |b| *b).unwrap();
        assert_eq!(block1_empty_list.occupied_count(), 1);
        assert_eq!(block1_empty_list.array(0).len(), 0);

        assert!(engine.del(b"recycle_key").unwrap());
        assert_eq!(engine.total_entries(), 0);

        let block1_deleted = engine.mmap.read_exact::<AdjArrayBlock, _, _>(1, |b| *b).unwrap();
        assert_eq!(block1_deleted.occupied_count(), 0);
        assert!(!block1_deleted.is_array_occupied(0));

        let _ = std::fs::remove_file(&temp.versioned);
    }

    #[test]
    fn err_engine_capacity_exhaustion() {
        let temp = TempFile::new("err_engine_cap_exhaustion");
        let cfg = TurboCfg::new(&temp.path);
        let engine = Engine::create(cfg).unwrap();

        assert!(engine.ensure_page_capacity(1).is_ok());
        let remaining = engine.remaining_pages().unwrap();
        assert!(engine.ensure_page_capacity(remaining).is_ok());
        assert!(matches!(
            engine.ensure_page_capacity(remaining + 1),
            Err(TurboError::DatabaseFull)
        ));

        let _ = std::fs::remove_file(&temp.versioned);
    }
}
