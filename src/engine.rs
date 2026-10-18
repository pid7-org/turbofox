use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use frozen_core::ack::AckTicket;
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

const INITIAL_PAGES: u32 = 8;
const INITIAL_SLOTS: usize = 0x0100;
const MIN_FILE_LEN: usize = (INITIAL_PAGES as usize) * (PAGE_SIZE as usize);

const ROOT_NODE_SLOT: usize = (Metadata::DEFAULT_ROOT_NODE_VP as usize) * 2;
const FIRST_BUCKET_VP: u32 = 4;
const LAST_BUCKET_VP: u32 = 0x43;

const DATABUF_SLOTS: [usize; 0x0E] =
    [0x88, 0x90, 0x98, 0xA0, 0xA8, 0xB0, 0xB8, 0xC8, 0xD0, 0xD8, 0xE0, 0xE8, 0xF0, 0xF8];

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

    /// Evaluates whether a bucket entry matches the target key bytes stored in DataBuf
    fn match_entry_key(&self, entry: &crate::bucket::BucketEntry, key: &[u8]) -> bool {
        if (entry.klen() as usize) != key.len() {
            return false;
        }
        let (dbuf_slot, local_idx) = DataBuf::decompose_buf_idx(entry.buffer_index());
        self.mmap
            .read_exact::<DataBuf, _, _>(dbuf_slot as usize, |dbuf| {
                match dbuf.read_kv_payload(local_idx, entry.klen() as usize, entry.vlen() as usize)
                {
                    Ok((k, _v)) => k == key,
                    Err(_) => false,
                }
            })
            .unwrap_or(false)
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
                self.mmap.read_exact::<DataBuf, _, _>(dbuf_slot as usize, |dbuf| {
                    let (_k, v) = dbuf.read_kv_payload(
                        local_idx,
                        entry.klen() as usize,
                        entry.vlen() as usize,
                    )?;
                    Ok(Some(v.to_vec()))
                })?
            }
            _ => Ok(None),
        }
    }

    /// Locates an active DataBuf in the freelist chain with contiguous buffer capacity
    fn find_free_databuf(
        &self,
        needed: usize,
    ) -> TurboResult<Option<(usize, usize, DataBuf, DataBuf)>> {
        let head = self.read_metadata()?.databuf_head as usize;
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
                return Ok(Some((curr, local_idx, orig, mutated)));
            }
            curr = next;
        }

        Ok(None)
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
    pub(crate) fn set(&self, key: &[u8], val: &[u8]) -> TurboResult<AckTicket> {
        let req = DataBuf::required_kv_buffers(key.len(), val.len());
        if req > DataBuf::BUF_COUNT {
            return Err(TurboError::PayloadTooLarge);
        }
        if key.len() > 0xFFFF || val.len() > 0xFFFF {
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

                        let mut new_bucket = bucket;
                        new_bucket.entry_mut(entry_idx).set_kv(
                            key.len() as u16,
                            val.len() as u16,
                            old_buf_idx,
                            hash,
                        );

                        let mut changes = Vec::with_capacity(0x0A);
                        for i in 0..2 {
                            let s = bucket_slot + i;
                            let o = bucket.as_slots()[i];
                            let n = new_bucket.as_slots()[i];
                            if o != n {
                                changes.push((s, o, n));
                            }
                        }
                        for i in 0..DataBuf::SLOT_COUNT {
                            let s = (old_dbuf_slot as usize) + i;
                            let o = orig_dbuf.as_slots()[i];
                            let n = mutated_dbuf.as_slots()[i];
                            if o != n {
                                changes.push((s, o, n));
                            }
                        }

                        changes.sort_by_key(|(s, _, _)| *s);
                        let mut tx = self.mmap.new_tx();
                        for (s, o, n) in &changes {
                            tx.write(o, n, *s)?;
                        }

                        match tx.commit() {
                            Ok(ticket) => return Ok(ticket),
                            Err(_) => {
                                Self::occ_backoff(attempt);
                                continue;
                            }
                        }
                    } else {
                        let orig_old_dbuf = self
                            .mmap
                            .read_exact::<DataBuf, _, _>(old_dbuf_slot as usize, |d| *d)?;
                        let mut freed_old_dbuf = orig_old_dbuf;
                        freed_old_dbuf.free_contiguous(old_local_idx, old_req);

                        let alloc_res = if let Some(idx) = freed_old_dbuf.alloc_contiguous(req) {
                            Some((old_dbuf_slot as usize, idx, orig_old_dbuf, freed_old_dbuf))
                        } else {
                            match self.find_free_databuf(req)? {
                                Some(res) => Some(res),
                                None => {
                                    self.grow_databufs(req)?;
                                    self.find_free_databuf(req)?
                                }
                            }
                        };

                        let (new_dbuf_slot, new_local_idx, orig_new_dbuf, mut mutated_new_dbuf) =
                            match alloc_res {
                                Some(res) => res,
                                None => {
                                    Self::occ_backoff(attempt);
                                    continue;
                                }
                            };

                        if let Err(e) = mutated_new_dbuf.write_kv_payload(new_local_idx, key, val) {
                            return Err(TurboError::from(e));
                        }

                        let new_buf_idx =
                            DataBuf::global_buf_idx(new_dbuf_slot as u32, new_local_idx);
                        let mut new_bucket = bucket;
                        new_bucket.entry_mut(entry_idx).set_kv(
                            key.len() as u16,
                            val.len() as u16,
                            new_buf_idx,
                            hash,
                        );

                        let mut changes = Vec::with_capacity(0x12);
                        for i in 0..2 {
                            let s = bucket_slot + i;
                            let o = bucket.as_slots()[i];
                            let n = new_bucket.as_slots()[i];
                            if o != n {
                                changes.push((s, o, n));
                            }
                        }

                        if new_dbuf_slot == (old_dbuf_slot as usize) {
                            for i in 0..DataBuf::SLOT_COUNT {
                                let s = new_dbuf_slot + i;
                                let o = orig_old_dbuf.as_slots()[i];
                                let n = mutated_new_dbuf.as_slots()[i];
                                if o != n {
                                    changes.push((s, o, n));
                                }
                            }
                        } else {
                            for i in 0..DataBuf::SLOT_COUNT {
                                let s = (old_dbuf_slot as usize) + i;
                                let o = orig_old_dbuf.as_slots()[i];
                                let n = freed_old_dbuf.as_slots()[i];
                                if o != n {
                                    changes.push((s, o, n));
                                }
                            }
                            for i in 0..DataBuf::SLOT_COUNT {
                                let s = new_dbuf_slot + i;
                                let o = orig_new_dbuf.as_slots()[i];
                                let n = mutated_new_dbuf.as_slots()[i];
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
                            Ok(ticket) => return Ok(ticket),
                            Err(_) => {
                                Self::occ_backoff(attempt);
                                continue;
                            }
                        }
                    }
                }
                ProbeResult::Vacant(entry_idx) => {
                    let alloc_res = match self.find_free_databuf(req)? {
                        Some(res) => Some(res),
                        None => {
                            self.grow_databufs(req)?;
                            self.find_free_databuf(req)?
                        }
                    };

                    let (new_dbuf_slot, new_local_idx, orig_new_dbuf, mut mutated_new_dbuf) =
                        match alloc_res {
                            Some(res) => res,
                            None => {
                                Self::occ_backoff(attempt);
                                continue;
                            }
                        };

                    if let Err(e) = mutated_new_dbuf.write_kv_payload(new_local_idx, key, val) {
                        return Err(TurboError::from(e));
                    }

                    let new_buf_idx = DataBuf::global_buf_idx(new_dbuf_slot as u32, new_local_idx);
                    let mut new_bucket = bucket;
                    new_bucket.entry_mut(entry_idx).set_kv(
                        key.len() as u16,
                        val.len() as u16,
                        new_buf_idx,
                        hash,
                    );

                    let mut changes = Vec::with_capacity(0x0A);
                    for i in 0..2 {
                        let s = bucket_slot + i;
                        let o = bucket.as_slots()[i];
                        let n = new_bucket.as_slots()[i];
                        if o != n {
                            changes.push((s, o, n));
                        }
                    }
                    for i in 0..DataBuf::SLOT_COUNT {
                        let s = new_dbuf_slot + i;
                        let o = orig_new_dbuf.as_slots()[i];
                        let n = mutated_new_dbuf.as_slots()[i];
                        if o != n {
                            changes.push((s, o, n));
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
                            return Ok(ticket);
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
}
