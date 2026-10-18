use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use frozen_core::memmap::{MemMap, MemMapCfg, PageSize};

use crate::adjarr::AdjArrayBlock;
use crate::bucket::{Bucket, ProbeResult};
use crate::cfg::{MODULE_ID, PAGE_SIZE, TurboCfg};
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

#[derive(Clone, Debug)]
pub(crate) struct DbufAllocPlan {
    pub(crate) start_slot: usize,
    pub(crate) local_idx: usize,
    pub(crate) touched: Vec<(usize, DataBuf, DataBuf)>,
}

/// Low-level storage engine coordinating memory mapping and disk topography
#[derive(Debug)]
pub(crate) struct Engine {
    mmap: MemMap<Slot>,
    growth_mutex: Mutex<()>,
    total_slots: AtomicU32,
    total_entries: AtomicU64,
}

impl Engine {
    /// Initializes brand new database layout backed by MemMap;
    /// fails if backing file already exists
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

    /// Opens existing database file and verifies all geometric and structural invariants;
    /// fails if backing file does not exist
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

    /// Explicitly flushes mapped dirty pages to storage media
    #[inline]
    pub(crate) fn sync(&self) -> TurboResult<()> {
        let entries = self.total_entries.load(Ordering::Acquire);
        let _ = self.mmap.write_exact::<Metadata, _, _>(0, |m| {
            m.total_entries = entries;
        });
        self.mmap.sync()?;
        Ok(())
    }

    /// Read metadata record from slot 0
    #[inline]
    pub(crate) fn read_metadata(&self) -> TurboResult<Metadata> {
        let meta = self.mmap.read_exact::<Metadata, _, _>(0, |m| *m)?;
        Ok(meta)
    }

    /// Total operational slots allocated in the mapping
    #[inline]
    pub(crate) fn total_slots(&self) -> u32 {
        self.total_slots.load(Ordering::Acquire)
    }

    /// Reference to growth mutex coordinating multi-transaction expansion
    #[inline]
    pub(crate) fn growth_mutex(&self) -> &Mutex<()> {
        &self.growth_mutex
    }

    /// Underlying memory-mapped array reference
    #[inline]
    pub(crate) fn mmap(&self) -> &MemMap<Slot> {
        &self.mmap
    }

    /// Total active key-value and list entries in the database
    #[inline]
    pub(crate) fn total_entries(&self) -> u64 {
        self.total_entries.load(Ordering::Acquire)
    }

    /// Traverses the 0x40-way Trie down to target Bucket and returns (bucket_slot, depth)
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

    /// Evaluates whether a bucket entry matches the target key bytes stored in DataBuf
    fn match_entry_key(&self, entry: &crate::bucket::BucketEntry, key: &[u8]) -> bool {
        if (entry.klen() as usize) != key.len() {
            return false;
        }
        let (dbuf_slot, local_idx) = DataBuf::decompose_buf_idx(entry.buffer_index());
        let req = DataBuf::required_kv_buffers(entry.klen() as usize, entry.vlen() as usize);
        if local_idx + req <= DataBuf::BUF_COUNT {
            return self
                .mmap
                .read_exact::<DataBuf, _, _>(dbuf_slot as usize, |dbuf| {
                    match dbuf.read_kv_payload(
                        local_idx,
                        entry.klen() as usize,
                        entry.vlen() as usize,
                    ) {
                        Ok((k, _v)) => k == key,
                        Err(_) => false,
                    }
                })
                .unwrap_or(false);
        }

        let klen = entry.klen() as usize;
        let vlen = entry.vlen() as usize;
        let total_bytes = req * DataBuf::BUF_SIZE;
        let mut frame = vec![0u8; total_bytes];
        let mut dest_offset = 0;
        let mut remaining_buffers = req;
        let mut curr_slot = dbuf_slot as usize;
        let mut is_first = true;

        while remaining_buffers > 0 {
            let curr_local = if is_first { local_idx } else { 0 };
            let count = (DataBuf::BUF_COUNT - curr_local).min(remaining_buffers);
            let src_start = curr_local * DataBuf::BUF_SIZE;
            let bytes_len = count * DataBuf::BUF_SIZE;

            let read_res = self.mmap.read_exact::<DataBuf, _, _>(curr_slot, |dbuf| {
                let raw = dbuf.payload_bytes();
                frame[dest_offset..dest_offset + bytes_len]
                    .copy_from_slice(&raw[src_start..src_start + bytes_len]);
            });
            if read_res.is_err() {
                return false;
            }

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
            return false;
        }

        let k = &data[..klen];
        k == key
    }

    /// Retrieves value payload for the specified key
    pub(crate) fn get(&self, key: &[u8]) -> TurboResult<Option<Vec<u8>>> {
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
    ) -> TurboResult<(usize, bool, Vec<(usize, DataBuf, DataBuf)>)> {
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

    /// Locates an active DataBuf in the freelist chain with contiguous buffer capacity
    fn find_free_databuf(&self, needed: usize) -> TurboResult<Option<DbufAllocPlan>> {
        let head = self.read_metadata()?.databuf_head as usize;
        if head == 0 {
            return Ok(None);
        }

        if needed <= DataBuf::BUF_COUNT {
            let mut curr = head;
            while curr != 0 {
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

        let num_dbufs = (needed + DataBuf::BUF_COUNT - 1) / DataBuf::BUF_COUNT;
        let mut curr = head;

        while curr != 0 {
            let (next, fits, touched) = self.check_contiguous_dbuf_fit(curr, num_dbufs, needed)?;
            if fits {
                return Ok(Some(DbufAllocPlan {
                    start_slot: curr,
                    local_idx: 0,
                    touched,
                }));
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
        use std::collections::BTreeMap;
        let mut map: BTreeMap<usize, (DataBuf, DataBuf)> = BTreeMap::new();
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

        let pages = if needed <= 0xFC { 1 } else { (needed + 0xFB) / 0xFC };
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

    /// Deepens the Trie by splitting a full Bucket into a Node with 0x40 child Buckets (§5.2)
    fn split_bucket(&self, bucket_slot: usize, depth: u8) -> TurboResult<()> {
        let _guard = self.growth_mutex.lock().unwrap();

        let (is_bucket, is_full) =
            self.mmap.read_exact::<Bucket, _, _>(bucket_slot, |b| (b.is_bucket(), b.is_full()))?;
        if !is_bucket || !is_full {
            return Ok(());
        }

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

        for i in 0..0x40 {
            let child_slot = ((base_child_vp + (i as u32)) * 2) as usize;
            self.mmap.write_exact::<Bucket, _, _>(child_slot, |b| *b = child_buckets[i])?;
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

    /// Exponential backoff with pseudo-random jitter under OCC contention
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

    /// Inserts or updates a key-value pair using atomic OCC transaction commit
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
                        let s = bucket_slot + i;
                        let o = bucket.as_slots()[i];
                        let n = new_bucket.as_slots()[i];
                        if o != n {
                            changes.push((s, o, n));
                        }
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
                    let new_buf_idx = DataBuf::global_buf_idx(
                        alloc_res.start_slot as u32,
                        alloc_res.local_idx,
                    );
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
                        let s = bucket_slot + i;
                        let o = bucket.as_slots()[i];
                        let n = new_bucket.as_slots()[i];
                        if o != n {
                            changes.push((s, o, n));
                        }
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

    /// Consumes engine, unmaps memory, terminates worker, and deletes backing file
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

        // Update in-place with smaller payload
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

        // Seed with baseline key
        engine.set(b"target_expand", b"small").unwrap();

        // Saturate initial databufs with bulk allocations
        for i in 0..0x64 {
            let k = format!("fill_key_{:04x}", i);
            let v = vec![(i & 0xFF) as u8; 0x70];
            engine.set(k.as_bytes(), &v).unwrap();
        }

        // Expand target_expand to require large contiguous allocation
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

        // 0x0800 bytes payload spanning multiple consecutive DataBufs
        let large_val = vec![0xAB; 0x0800];
        engine.set(b"large_key", &large_val).unwrap();
        assert_eq!(engine.get(b"large_key").unwrap(), Some(large_val));

        // Update expansion: 0x1200 bytes spanning across ~5 DataBufs
        let huge_val = vec![0xCD; 0x1200];
        engine.set(b"large_key", &huge_val).unwrap();
        assert_eq!(engine.get(b"large_key").unwrap(), Some(huge_val));

        // Update shrink: 0x0200 bytes
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

        // Explicit sync flushes metadata slot 0
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
}
