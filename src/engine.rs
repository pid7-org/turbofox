use crate::adjarr::{AdjArrayBlock, decompose_relative_array_idx, relative_array_idx};
use crate::bucket::{Bucket, BucketEntry, ProbeResult};
use crate::cfg::TurboCfg;
use crate::common::{
    ADJ_ARR_HEAD_SLOT, ADJ_ARRAY_ENTRIES, ALLOC_SLOT_QUANTUM, BUCKET_SLOTS, DATABUF_BUF_COUNT,
    DATABUF_HEAD_SLOT, DATABUF_SLOTS, DB_NAME_TAG, INITIAL_ADJ_ARR_BLOCKS,
    INITIAL_FREE_SLOT_MARKER, LAYOUT_VERSION, MAGIC, MAX_DB_LEN, MAX_KEY_LEN, MAX_VAL_LEN,
    MODULE_ID, ROOT_BUCKET_SLOT, SLOT_SIZE, Slot,
};
use crate::dbuf::DataBuf;
use crate::error::{TurboError, TurboResult};
use crate::hash::hash_key;
use crate::meta::Metadata;
use crate::ticket::TurboTicket;
use frozen_core::memmap::{MemMap, MemMapCfg, PageSize};
use std::sync::{
    Mutex,
    atomic::{AtomicU32, AtomicU64, Ordering},
};

#[derive(Clone, Debug)]
pub(crate) struct DbufAllocPlan {
    start_slot: usize,
    local_idx: usize,
    orig: DataBuf,
    mutated: DataBuf,
}

#[derive(Debug)]
pub(crate) struct Engine {
    mmap: MemMap<Slot>,
    growth_mutex: Mutex<()>,
    total_slots: AtomicU32,
    total_entries: AtomicU64,
    free_slot_marker: AtomicU32,
    active_dbuf_hint: AtomicU32,
    sys_page_size: PageSize,
}

impl Engine {
    /// Ref: `DESIGN.md` §8.1
    pub(crate) fn create(cfg: TurboCfg) -> TurboResult<Self> {
        let page_size = MemMap::<Slot>::page_size()?;
        let initial_pages = match page_size {
            PageSize::K4 => 4,
            PageSize::K16 | PageSize::K64 => 1,
        };

        let dir_path = cfg.resolve_dir();
        let memmap_cfg = MemMapCfg {
            module_id: MODULE_ID,
            path: dir_path,
            name: DB_NAME_TAG,
            version: LAYOUT_VERSION,
            initial_pages,
            max_len: MAX_DB_LEN,
            sync_interval: cfg.sync_interval,
            memory_priority_hint: true,
            sequential_access_hint: true,
        };

        let mmap = MemMap::<Slot>::new(memmap_cfg)?;
        let initial_slots = (initial_pages * page_size.as_usize()) / SLOT_SIZE;

        let engine = Self {
            mmap,
            growth_mutex: Mutex::new(()),
            total_slots: AtomicU32::new(initial_slots as u32),
            total_entries: AtomicU64::new(0),
            free_slot_marker: AtomicU32::new(INITIAL_FREE_SLOT_MARKER),
            active_dbuf_hint: AtomicU32::new(DATABUF_HEAD_SLOT),
            sys_page_size: page_size,
        };

        engine.format_initial_topography(LAYOUT_VERSION, initial_pages as u32)?;
        engine.sync()?;

        Ok(engine)
    }

    /// Ref: `DESIGN.md` §8.1
    pub(crate) fn open(cfg: TurboCfg) -> TurboResult<Self> {
        let sys_page_size = MemMap::<Slot>::page_size()?;
        let initial_pages = match sys_page_size {
            PageSize::K4 => 4,
            PageSize::K16 | PageSize::K64 => 1,
        };

        let dir_path = cfg.resolve_dir();
        let memmap_cfg = MemMapCfg {
            module_id: MODULE_ID,
            path: dir_path,
            name: DB_NAME_TAG,
            version: LAYOUT_VERSION,
            initial_pages,
            max_len: MAX_DB_LEN,
            sync_interval: cfg.sync_interval,
            memory_priority_hint: true,
            sequential_access_hint: true,
        };

        let mmap = MemMap::<Slot>::open(memmap_cfg)?;
        let file_len = mmap.length();

        if file_len % sys_page_size.as_usize() != 0 {
            return Err(TurboError::InvalidSize {
                expected: (file_len / sys_page_size.as_usize()) * sys_page_size.as_usize(),
                found: file_len,
            });
        }

        let meta = mmap.read_exact::<Metadata, _, _>(0, |m| *m)?;
        if meta.magic != MAGIC {
            return Err(TurboError::InvalidMagic);
        }

        if meta.version != LAYOUT_VERSION {
            return Err(TurboError::VersionMismatch {
                expected: LAYOUT_VERSION,
                found: meta.version,
            });
        }

        if meta.page_size != sys_page_size.as_u32() {
            return Err(TurboError::InvalidPageSize(meta.page_size));
        }

        if !meta.validate(sys_page_size.as_u32(), LAYOUT_VERSION) {
            return Err(TurboError::CorruptedHeader("metadata validation failed"));
        }

        let expected_slots = (file_len / SLOT_SIZE) as u32;
        if meta.total_slots != expected_slots {
            return Err(TurboError::CorruptedHeader("metadata total_slots divergence"));
        }

        let expected_pages = (file_len / sys_page_size.as_usize()) as u32;
        if meta.total_pages != expected_pages {
            return Err(TurboError::CorruptedHeader("metadata total_pages divergence"));
        }

        let engine = Self {
            mmap,
            growth_mutex: Mutex::new(()),
            total_slots: AtomicU32::new(meta.total_slots),
            total_entries: AtomicU64::new(meta.total_entries),
            free_slot_marker: AtomicU32::new(meta.free_slot_marker),
            active_dbuf_hint: AtomicU32::new(DATABUF_HEAD_SLOT),
            sys_page_size,
        };

        Ok(engine)
    }

    /// Ref: `DESIGN.md` §2
    fn format_initial_topography(&self, version: u32, initial_pages: u32) -> TurboResult<()> {
        let meta = Metadata::new(self.sys_page_size.as_u32(), version, initial_pages);
        self.mmap.write_exact::<Metadata, _, _>(0, |m| *m = meta)?;

        for slot in 1..INITIAL_ADJ_ARR_BLOCKS {
            let mut block = AdjArrayBlock::new();
            block.set_next_block((slot + 1) as u32);
            self.mmap.write_exact::<AdjArrayBlock, _, _>(slot, |b| *b = block)?;
        }

        let block_tail = AdjArrayBlock::new();
        self.mmap
            .write_exact::<AdjArrayBlock, _, _>(INITIAL_ADJ_ARR_BLOCKS, |b| *b = block_tail)?;

        let root_bucket = Bucket::new();
        self.mmap.write_exact::<Bucket, _, _>(ROOT_BUCKET_SLOT as usize, |b| *b = root_bucket)?;

        let mut dbuf0 = DataBuf::new();
        dbuf0.set_next_dbuf_index(DATABUF_HEAD_SLOT + (DATABUF_SLOTS as u32));
        self.mmap.write_exact::<DataBuf, _, _>(DATABUF_HEAD_SLOT as usize, |d| *d = dbuf0)?;

        let dbuf1 = DataBuf::new();
        self.mmap
            .write_exact::<DataBuf, _, _>((DATABUF_HEAD_SLOT as usize) + DATABUF_SLOTS, |d| {
                *d = dbuf1
            })?;

        Ok(())
    }

    fn alloc_uniform_slots_locked(&self, count: u32) -> TurboResult<u32> {
        let current_free = self.free_slot_marker.load(Ordering::Acquire);
        let total = self.total_slots.load(Ordering::Acquire);

        if current_free.saturating_add(count) > total {
            self.grow_backing_file(count)?;
        }

        let allocated = current_free;
        let next_free = current_free + count;

        self.free_slot_marker.store(next_free, Ordering::Release);
        self.mmap.write_exact::<Metadata, _, _>(0, |m| {
            m.free_slot_marker = next_free;
        })?;

        Ok(allocated)
    }

    /// Ref: `DESIGN.md` §5.1
    pub(crate) fn alloc_uniform_slots(&self, count: u32) -> TurboResult<u32> {
        let _guard = self.growth_mutex.lock().unwrap_or_else(|e| e.into_inner());
        self.alloc_uniform_slots_locked(count)
    }

    /// Ref: `DESIGN.md` §5.4
    fn grow_backing_file(&self, needed_slots: u32) -> TurboResult<()> {
        let curr_len = self.mmap.length();
        let page_bytes = self.sys_page_size.as_usize();

        let quantum_pages = match self.sys_page_size {
            PageSize::K4 => 4,
            PageSize::K16 | PageSize::K64 => 1,
        };

        let needed_pages = ((needed_slots as usize * SLOT_SIZE) + page_bytes - 1) / page_bytes;
        let growth_pages = ((needed_pages + quantum_pages - 1) / quantum_pages) * quantum_pages;
        let added_bytes = growth_pages * page_bytes;

        if curr_len.saturating_add(added_bytes) > MAX_DB_LEN {
            return Err(TurboError::DatabaseFull);
        }

        self.mmap.grow(growth_pages)?;

        let mut new_total_slots = 0u32;
        self.mmap.write_exact::<Metadata, _, _>(0, |m| {
            m.record_growth(growth_pages as u32);
            new_total_slots = m.total_slots;
        })?;

        self.total_slots.store(new_total_slots, Ordering::Release);
        Ok(())
    }

    /// Ref: `DESIGN.md` §5.3
    pub(crate) fn expand_bucket(&self) -> TurboResult<u32> {
        let new_slot = self.alloc_uniform_slots(BUCKET_SLOTS as u32)?;
        let new_bucket = Bucket::new();

        self.mmap.write_exact::<Bucket, _, _>(new_slot as usize, |b| *b = new_bucket)?;

        Ok(new_slot)
    }

    fn expand_databuf_locked(&self, tail_slot: usize) -> TurboResult<u32> {
        let new_slot = self.alloc_uniform_slots_locked(DATABUF_SLOTS as u32)?;
        let new_dbuf = DataBuf::new();

        self.mmap.write_exact::<DataBuf, _, _>(new_slot as usize, |d| *d = new_dbuf)?;
        self.mmap.write_exact::<DataBuf, _, _>(tail_slot, |d| {
            d.set_next_dbuf_index(new_slot);
        })?;

        Ok(new_slot)
    }

    /// Ref: `DESIGN.md` §5.3
    #[allow(dead_code)]
    pub(crate) fn expand_databuf(&self, tail_slot: usize) -> TurboResult<u32> {
        let _guard = self.growth_mutex.lock().unwrap_or_else(|e| e.into_inner());
        let new_slot = self.expand_databuf_locked(tail_slot)?;

        self.active_dbuf_hint.store(new_slot, Ordering::Relaxed);

        Ok(new_slot)
    }

    /// Ref: `DESIGN.md` §5.3
    pub(crate) fn expand_adjarr(&self, tail_slot: usize) -> TurboResult<u32> {
        let new_slot = self.alloc_uniform_slots(ALLOC_SLOT_QUANTUM)?;
        for i in 0..ALLOC_SLOT_QUANTUM {
            let s = (new_slot + i) as usize;
            let next = if i + 1 < ALLOC_SLOT_QUANTUM { (new_slot + i + 1) as u32 } else { 0 };
            let mut block = AdjArrayBlock::new();
            block.set_next_block(next);
            self.mmap.write_exact::<AdjArrayBlock, _, _>(s, |b| *b = block)?;
        }

        self.mmap.write_exact::<AdjArrayBlock, _, _>(tail_slot, |b| {
            b.set_next_block(new_slot);
        })?;

        Ok(new_slot)
    }

    /// Ref: `DESIGN.md` §3.3
    pub(crate) fn locate_databuf_slot(&self, pool_hop: u32) -> TurboResult<u32> {
        let mut curr = DATABUF_HEAD_SLOT;
        for _ in 0..pool_hop {
            let next =
                self.mmap.read_exact::<DataBuf, _, _>(curr as usize, |d| d.next_dbuf_index())?;

            if next == 0 {
                return Err(TurboError::CorruptedHeader("broken databuf chain"));
            }

            curr = next;
        }

        Ok(curr)
    }

    /// Ref: `DESIGN.md` §3.4
    pub(crate) fn locate_adjarr_block_slot(&self, block_hop: u32) -> TurboResult<u32> {
        let mut curr = ADJ_ARR_HEAD_SLOT;
        for _ in 0..block_hop {
            let next =
                self.mmap.read_exact::<AdjArrayBlock, _, _>(curr as usize, |b| b.next_block())?;

            if next == 0 {
                return Err(TurboError::CorruptedHeader("broken adjarr block chain"));
            }

            curr = next;
        }

        Ok(curr)
    }

    /// Ref: `DESIGN.md` §3.3
    pub(crate) fn find_free_databuf(
        &self,
        needed: usize,
        exclude_slot: usize,
    ) -> TurboResult<DbufAllocPlan> {
        let hint = self.active_dbuf_hint.load(Ordering::Relaxed) as usize;
        let start_slot = if hint == 0 { DATABUF_HEAD_SLOT as usize } else { hint };

        let mut curr = start_slot;
        let mut tail_slot = start_slot;
        let mut wrapped = false;

        loop {
            if curr == exclude_slot {
                let next = self
                    .mmap
                    .read_exact::<DataBuf, _, _>(curr, |d| d.next_dbuf_index() as usize)?;

                if next != 0 {
                    curr = next;
                } else {
                    tail_slot = curr;
                    if wrapped || start_slot == (DATABUF_HEAD_SLOT as usize) {
                        break;
                    }
                    wrapped = true;
                    curr = DATABUF_HEAD_SLOT as usize;
                }
            } else {
                let (next, alloc_opt) = self.mmap.read_exact::<DataBuf, _, _>(curr, |d| {
                    let next = d.next_dbuf_index() as usize;
                    let mut mutated = *d;

                    match mutated.alloc_contiguous(needed) {
                        Some(local_idx) => (next, Some((local_idx, *d, mutated))),
                        None => (next, None),
                    }
                })?;

                if let Some((local_idx, orig, mutated)) = alloc_opt {
                    if wrapped {
                        self.active_dbuf_hint.store(curr as u32, Ordering::Relaxed);
                    }
                    return Ok(DbufAllocPlan { start_slot: curr, local_idx, orig, mutated });
                }

                if next != 0 {
                    if !wrapped {
                        self.active_dbuf_hint.store(next as u32, Ordering::Relaxed);
                    }
                    curr = next;
                } else {
                    tail_slot = curr;
                    if wrapped || start_slot == (DATABUF_HEAD_SLOT as usize) {
                        break;
                    }
                    wrapped = true;
                    curr = DATABUF_HEAD_SLOT as usize;
                }
            }

            if wrapped && curr == start_slot {
                break;
            }
        }

        let _guard = self.growth_mutex.lock().unwrap_or_else(|e| e.into_inner());

        let mut actual_tail = tail_slot;
        loop {
            let next = self
                .mmap
                .read_exact::<DataBuf, _, _>(actual_tail, |d| d.next_dbuf_index() as usize)?;
            if next == 0 {
                break;
            }
            actual_tail = next;
            if actual_tail != exclude_slot {
                let (_, alloc_opt) = self.mmap.read_exact::<DataBuf, _, _>(actual_tail, |d| {
                    let next = d.next_dbuf_index() as usize;
                    let mut mutated = *d;
                    match mutated.alloc_contiguous(needed) {
                        Some(local_idx) => (next, Some((local_idx, *d, mutated))),
                        None => (next, None),
                    }
                })?;
                if let Some((local_idx, orig, mutated)) = alloc_opt {
                    self.active_dbuf_hint.store(actual_tail as u32, Ordering::Relaxed);
                    return Ok(DbufAllocPlan { start_slot: actual_tail, local_idx, orig, mutated });
                }
            }
        }

        let new_slot = self.expand_databuf_locked(actual_tail)? as usize;
        self.active_dbuf_hint.store(new_slot as u32, Ordering::Relaxed);

        let mut dbuf = self.mmap.read_exact::<DataBuf, _, _>(new_slot, |d| *d)?;
        let orig = dbuf;
        let local_idx = dbuf.alloc_contiguous(needed).ok_or(TurboError::DatabaseFull)?;

        Ok(DbufAllocPlan { start_slot: new_slot, local_idx, orig, mutated: dbuf })
    }

    /// Ref: `DESIGN.md` §3.3
    pub(crate) fn databuf_slot_to_hop(&self, slot: u32) -> TurboResult<u32> {
        let mut curr = DATABUF_HEAD_SLOT;
        let mut hop = 0;

        while curr != 0 {
            if curr == slot {
                return Ok(hop);
            }

            curr = self.mmap.read_exact::<DataBuf, _, _>(curr as usize, |d| d.next_dbuf_index())?;
            hop += 1;
        }

        Err(TurboError::CorruptedHeader("databuf slot not found in allocation chain"))
    }

    /// Ref: `DESIGN.md` §6
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

    /// Ref: `DESIGN.md` §7.1
    pub(crate) fn get(&self, key: &[u8]) -> TurboResult<Option<Vec<u8>>> {
        if key.is_empty() || key.len() > MAX_KEY_LEN {
            return Ok(None);
        }

        let hash = hash_key(key);
        let mut bucket_slot = ROOT_BUCKET_SLOT;
        let mut depth = 0u8;

        loop {
            let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;
            let probe_res = bucket
                .probe(hash, depth, |entry_idx| self.match_any_key(&bucket.entry(entry_idx), key));

            match probe_res {
                ProbeResult::Found(entry_idx) => {
                    let entry = bucket.entry(entry_idx);
                    if entry.is_list() {
                        return Err(TurboError::InvalidType("Key is a list"));
                    }

                    let (pool_hop, local_idx) = DataBuf::decompose_buf_idx(entry.buf_idx());
                    let dbuf_slot = self.locate_databuf_slot(pool_hop)?;
                    let dbuf = self.mmap.read_exact::<DataBuf, _, _>(dbuf_slot as usize, |d| *d)?;

                    let (_, val) = dbuf
                        .read_kv_payload(local_idx, entry.klen() as usize, entry.vlen() as usize)
                        .map_err(|_| TurboError::CorruptedPayload("KV CRC32 mismatch"))?;

                    return Ok(Some(val.to_vec()));
                }
                ProbeResult::Vacant(_) => return Ok(None),
                ProbeResult::Overflow { next_bucket, .. } => {
                    if next_bucket == 0 {
                        return Ok(None);
                    }

                    bucket_slot = next_bucket;
                    depth = depth.wrapping_add(1);
                }
            }
        }
    }

    fn match_any_key(&self, entry: &BucketEntry, key: &[u8]) -> bool {
        if entry.is_list() {
            self.match_list_key(entry, key)
        } else {
            self.match_kv_key(entry, key)
        }
    }

    fn match_kv_key(&self, entry: &BucketEntry, key: &[u8]) -> bool {
        if entry.is_list() || (entry.klen() as usize) != key.len() {
            return false;
        }

        let (pool_hop, local_idx) = DataBuf::decompose_buf_idx(entry.buf_idx());
        let Ok(dbuf_slot) = self.locate_databuf_slot(pool_hop) else {
            return false;
        };

        let Ok(matches) = self
            .mmap
            .read_exact::<DataBuf, _, _>(dbuf_slot as usize, |d| d.verify_key(local_idx, key))
        else {
            return false;
        };

        matches
    }

    /// Ref: `DESIGN.md` §7.2
    pub(crate) fn set(&self, key: &[u8], val: &[u8]) -> TurboResult<TurboTicket> {
        if key.is_empty() || key.len() > MAX_KEY_LEN || val.len() > MAX_VAL_LEN {
            return Err(TurboError::PayloadTooLarge);
        }

        let req = DataBuf::required_kv_buffers(key.len(), val.len());
        if req > DATABUF_BUF_COUNT {
            return Err(TurboError::PayloadTooLarge);
        }

        let hash = hash_key(key);

        for attempt in 0..0x20 {
            let mut bucket_slot = ROOT_BUCKET_SLOT;
            let mut depth = 0u8;

            let probe_info = loop {
                let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;
                let probe_res = bucket.probe(hash, depth, |entry_idx| {
                    self.match_any_key(&bucket.entry(entry_idx), key)
                });

                match probe_res {
                    ProbeResult::Found(entry_idx) => {
                        break Ok((bucket_slot, bucket, entry_idx, true));
                    }
                    ProbeResult::Vacant(entry_idx) => {
                        break Ok((bucket_slot, bucket, entry_idx, false));
                    }
                    ProbeResult::Overflow { primary, next_bucket, candidate_slot } => {
                        if next_bucket == 0 {
                            if let Some(tomb_idx) = candidate_slot {
                                break Ok((bucket_slot, bucket, tomb_idx, false));
                            }
                            let new_bucket_slot = self.expand_bucket()?;
                            self.mmap.write_exact::<Bucket, _, _>(bucket_slot as usize, |b| {
                                b.set_next_bucket(primary, new_bucket_slot);
                            })?;
                            bucket_slot = new_bucket_slot;
                            depth = depth.wrapping_add(1);
                        } else {
                            bucket_slot = next_bucket;
                            depth = depth.wrapping_add(1);
                        }
                    }
                }
            };

            let (target_bucket_slot, old_bucket, target_entry_idx, is_update) = match probe_info {
                Ok(info) => info,
                Err(e) => return Err(e),
            };

            let mut touched_dbufs: Vec<(usize, DataBuf, DataBuf)> = Vec::with_capacity(2);
            let target_global_buf: u32;

            if is_update {
                let old_entry = old_bucket.entry(target_entry_idx);
                if old_entry.is_list() {
                    return Err(TurboError::InvalidType("cannot overwrite list key with KV"));
                }

                let old_req = DataBuf::required_kv_buffers(
                    old_entry.klen() as usize,
                    old_entry.vlen() as usize,
                );
                let (old_pool_hop, old_local_idx) = DataBuf::decompose_buf_idx(old_entry.buf_idx());
                let old_dbuf_slot = self.locate_databuf_slot(old_pool_hop)? as usize;
                let orig_old_dbuf = self.mmap.read_exact::<DataBuf, _, _>(old_dbuf_slot, |d| *d)?;

                if req <= old_req {
                    let mut mutated = orig_old_dbuf;
                    if req < old_req {
                        mutated.free_contiguous(old_local_idx + req, old_req - req);
                    }
                    mutated
                        .write_kv_payload(old_local_idx, key, val)
                        .map_err(|_| TurboError::PayloadTooLarge)?;

                    touched_dbufs.push((old_dbuf_slot, orig_old_dbuf, mutated));
                    target_global_buf = old_entry.buf_idx();
                } else {
                    let mut freed_old = orig_old_dbuf;
                    freed_old.free_contiguous(old_local_idx, old_req);

                    if let Some(new_local) = freed_old.alloc_contiguous(req) {
                        freed_old
                            .write_kv_payload(new_local, key, val)
                            .map_err(|_| TurboError::PayloadTooLarge)?;
                        target_global_buf = DataBuf::global_buf_idx(old_pool_hop, new_local);
                        touched_dbufs.push((old_dbuf_slot, orig_old_dbuf, freed_old));
                    } else {
                        let mut plan = self.find_free_databuf(req, old_dbuf_slot)?;
                        plan.mutated
                            .write_kv_payload(plan.local_idx, key, val)
                            .map_err(|_| TurboError::PayloadTooLarge)?;
                        let new_pool_hop = self.databuf_slot_to_hop(plan.start_slot as u32)?;
                        target_global_buf = DataBuf::global_buf_idx(new_pool_hop, plan.local_idx);

                        touched_dbufs.push((old_dbuf_slot, orig_old_dbuf, freed_old));
                        touched_dbufs.push((plan.start_slot, plan.orig, plan.mutated));
                    }
                }
            } else {
                let mut plan = self.find_free_databuf(req, 0)?;
                plan.mutated
                    .write_kv_payload(plan.local_idx, key, val)
                    .map_err(|_| TurboError::PayloadTooLarge)?;
                let pool_hop = self.databuf_slot_to_hop(plan.start_slot as u32)?;
                target_global_buf = DataBuf::global_buf_idx(pool_hop, plan.local_idx);
                touched_dbufs.push((plan.start_slot, plan.orig, plan.mutated));
            }

            let mut new_bucket = old_bucket;
            new_bucket.set_kv(
                target_entry_idx,
                hash,
                key.len() as u8,
                val.len() as u16,
                target_global_buf,
            );

            let mut changes: Vec<(usize, Slot, Slot)> =
                Vec::with_capacity(BUCKET_SLOTS + touched_dbufs.len() * DATABUF_SLOTS);

            for i in 0..BUCKET_SLOTS {
                let orig_s = old_bucket.as_slots()[i];
                let new_s = new_bucket.as_slots()[i];
                if orig_s != new_s {
                    changes.push(((target_bucket_slot as usize) + i, orig_s, new_s));
                }
            }

            for (dbuf_slot, orig_d, mut_d) in &touched_dbufs {
                for i in 0..DATABUF_SLOTS {
                    let orig_s = orig_d.as_slots()[i];
                    let new_s = mut_d.as_slots()[i];
                    if orig_s != new_s {
                        changes.push((dbuf_slot + i, orig_s, new_s));
                    }
                }
            }

            changes.sort_by_key(|(s, _, _)| *s);
            let mut tx = self.mmap.new_tx();
            for (s, orig_s, new_s) in &changes {
                if let Err(e) = tx.write(orig_s, new_s, *s) {
                    return Err(TurboError::from(e));
                }
            }

            match tx.commit() {
                Ok(ticket) => {
                    if !is_update {
                        self.total_entries.fetch_add(1, Ordering::Relaxed);
                    }
                    return Ok(TurboTicket::new(ticket));
                }
                Err(_) => {
                    Self::occ_backoff(attempt);
                    continue;
                }
            }
        }

        Err(TurboError::ResourceBusy("OCC write conflict limit reached"))
    }

    /// Ref: `DESIGN.md` §7.3
    pub(crate) fn del(&self, key: &[u8]) -> TurboResult<bool> {
        if key.is_empty() || key.len() > MAX_KEY_LEN {
            return Ok(false);
        }

        let hash = hash_key(key);

        for attempt in 0..0x20 {
            let mut bucket_slot = ROOT_BUCKET_SLOT;
            let mut depth = 0u8;

            let probe_res = loop {
                let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;
                let res = bucket.probe(hash, depth, |entry_idx| {
                    self.match_any_key(&bucket.entry(entry_idx), key)
                });

                match res {
                    ProbeResult::Found(entry_idx) => {
                        break Ok((bucket_slot, bucket, entry_idx));
                    }
                    ProbeResult::Vacant(_) => return Ok(false),
                    ProbeResult::Overflow { next_bucket, .. } => {
                        if next_bucket == 0 {
                            return Ok(false);
                        }
                        bucket_slot = next_bucket;
                        depth = depth.wrapping_add(1);
                    }
                }
            };

            let (target_bucket_slot, old_bucket, target_entry_idx) = match probe_res {
                Ok(res) => res,
                Err(e) => return Err(e),
            };

            let entry = old_bucket.entry(target_entry_idx);
            if entry.is_list() {
                return self.list_del(key);
            }

            let req = DataBuf::required_kv_buffers(entry.klen() as usize, entry.vlen() as usize);
            let (pool_hop, local_idx) = DataBuf::decompose_buf_idx(entry.buf_idx());
            let dbuf_slot = self.locate_databuf_slot(pool_hop)? as usize;
            let orig_dbuf = self.mmap.read_exact::<DataBuf, _, _>(dbuf_slot, |d| *d)?;

            let mut freed_dbuf = orig_dbuf;
            freed_dbuf.free_contiguous(local_idx, req);

            let mut new_bucket = old_bucket;
            new_bucket.mark_tombstone(target_entry_idx);

            let mut changes: Vec<(usize, Slot, Slot)> =
                Vec::with_capacity(BUCKET_SLOTS + DATABUF_SLOTS);

            for i in 0..BUCKET_SLOTS {
                let orig_s = old_bucket.as_slots()[i];
                let new_s = new_bucket.as_slots()[i];
                if orig_s != new_s {
                    changes.push(((target_bucket_slot as usize) + i, orig_s, new_s));
                }
            }

            for i in 0..DATABUF_SLOTS {
                let orig_s = orig_dbuf.as_slots()[i];
                let new_s = freed_dbuf.as_slots()[i];
                if orig_s != new_s {
                    changes.push((dbuf_slot + i, orig_s, new_s));
                }
            }

            changes.sort_by_key(|(s, _, _)| *s);
            let mut tx = self.mmap.new_tx();
            for (s, orig_s, new_s) in &changes {
                if let Err(e) = tx.write(orig_s, new_s, *s) {
                    return Err(TurboError::from(e));
                }
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

        Err(TurboError::ResourceBusy("OCC write conflict limit reached"))
    }

    fn match_list_key(&self, entry: &BucketEntry, key: &[u8]) -> bool {
        if !entry.is_list() {
            return false;
        }

        let (pool_hop, local_idx) = DataBuf::decompose_buf_idx(entry.buf_idx());
        let Ok(dbuf_slot) = self.locate_databuf_slot(pool_hop) else {
            return false;
        };

        let Ok(matches) = self
            .mmap
            .read_exact::<DataBuf, _, _>(dbuf_slot as usize, |d| d.verify_list_key(local_idx, key))
        else {
            return false;
        };

        matches
    }

    /// Ref: `DESIGN.md` §7.4
    pub(crate) fn list_append(&self, key: &[u8], item: &[u8]) -> TurboResult<TurboTicket> {
        if key.is_empty() || key.len() > MAX_KEY_LEN || item.len() > MAX_VAL_LEN {
            return Err(TurboError::PayloadTooLarge);
        }

        let req_item = DataBuf::required_list_val_buffers(item.len());
        if req_item > DATABUF_BUF_COUNT {
            return Err(TurboError::PayloadTooLarge);
        }

        let hash = hash_key(key);

        for attempt in 0..0x20 {
            let mut bucket_slot = ROOT_BUCKET_SLOT;
            let mut depth = 0u8;

            let probe_res = loop {
                let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;
                let res = bucket.probe(hash, depth, |entry_idx| {
                    self.match_any_key(&bucket.entry(entry_idx), key)
                });

                match res {
                    ProbeResult::Found(entry_idx) => {
                        break Ok((bucket_slot, bucket, entry_idx, true));
                    }
                    ProbeResult::Vacant(entry_idx) => {
                        break Ok((bucket_slot, bucket, entry_idx, false));
                    }
                    ProbeResult::Overflow { primary, next_bucket, candidate_slot } => {
                        if next_bucket == 0 {
                            if let Some(tomb_idx) = candidate_slot {
                                break Ok((bucket_slot, bucket, tomb_idx, false));
                            }
                            let new_bucket_slot = self.expand_bucket()?;
                            self.mmap.write_exact::<Bucket, _, _>(bucket_slot as usize, |b| {
                                b.set_next_bucket(primary, new_bucket_slot);
                            })?;
                            bucket_slot = new_bucket_slot;
                            depth = depth.wrapping_add(1);
                        } else {
                            bucket_slot = next_bucket;
                            depth = depth.wrapping_add(1);
                        }
                    }
                }
            };

            let (target_bucket_slot, old_bucket, target_entry_idx, exists) = match probe_res {
                Ok(res) => res,
                Err(e) => return Err(e),
            };

            if exists {
                let entry = old_bucket.entry(target_entry_idx);
                if !entry.is_list() {
                    return Err(TurboError::InvalidType("Key is a KV entry"));
                }

                let mut item_plan = self.find_free_databuf(req_item, 0)?;
                item_plan
                    .mutated
                    .write_list_val_payload(item_plan.local_idx, item)
                    .map_err(|_| TurboError::PayloadTooLarge)?;
                let item_pool_hop = self.databuf_slot_to_hop(item_plan.start_slot as u32)?;
                let item_buf_idx = DataBuf::global_buf_idx(item_pool_hop, item_plan.local_idx);

                let mut curr_arr_idx = entry.adjarr_idx();
                let (last_block_slot, last_orig_block, last_local_array) = loop {
                    let (block_hop, local_array) = decompose_relative_array_idx(curr_arr_idx);
                    let block_slot = self.locate_adjarr_block_slot(block_hop)? as usize;
                    let block = self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot, |b| *b)?;

                    let next = block.array(local_array).next_array();
                    if next == 0 {
                        break (block_slot, block, local_array);
                    }
                    curr_arr_idx = next;
                };

                let mut touched_blocks: Vec<(usize, AdjArrayBlock, AdjArrayBlock)> = Vec::new();

                if !last_orig_block.array(last_local_array).is_full() {
                    let mut mutated_block = last_orig_block;
                    mutated_block.array_mut(last_local_array).push(item_buf_idx);
                    touched_blocks.push((last_block_slot, last_orig_block, mutated_block));
                } else {
                    let (new_arr_idx, new_block_slot, new_orig_block, mut new_mut_block) =
                        self.find_free_adjarr_array(last_block_slot)?;

                    let (_, new_local_arr) = decompose_relative_array_idx(new_arr_idx);
                    new_mut_block.array_mut(new_local_arr).push(item_buf_idx);

                    let mut mutated_last_block = last_orig_block;
                    mutated_last_block.array_mut(last_local_array).set_next_array(new_arr_idx);

                    if last_block_slot == new_block_slot {
                        mutated_last_block.array_mut(new_local_arr).push(item_buf_idx);
                        mutated_last_block.set_array_occupied(new_local_arr, true);
                        touched_blocks.push((last_block_slot, last_orig_block, mutated_last_block));
                    } else {
                        touched_blocks.push((last_block_slot, last_orig_block, mutated_last_block));
                        touched_blocks.push((new_block_slot, new_orig_block, new_mut_block));
                    }
                }

                let mut changes: Vec<(usize, Slot, Slot)> = Vec::new();
                for (bs, orig_b, mut_b) in &touched_blocks {
                    let orig_s = *orig_b.as_slot();
                    let new_s = *mut_b.as_slot();
                    if orig_s != new_s {
                        changes.push((*bs, orig_s, new_s));
                    }
                }

                for i in 0..DATABUF_SLOTS {
                    let orig_s = item_plan.orig.as_slots()[i];
                    let new_s = item_plan.mutated.as_slots()[i];
                    if orig_s != new_s {
                        changes.push((item_plan.start_slot + i, orig_s, new_s));
                    }
                }

                changes.sort_by_key(|(s, _, _)| *s);
                let mut tx = self.mmap.new_tx();
                for (s, orig_s, new_s) in &changes {
                    if let Err(e) = tx.write(orig_s, new_s, *s) {
                        return Err(TurboError::from(e));
                    }
                }

                match tx.commit() {
                    Ok(ticket) => return Ok(TurboTicket::new(ticket)),
                    Err(_) => {
                        Self::occ_backoff(attempt);
                        continue;
                    }
                }
            } else {
                let req_key = DataBuf::required_list_key_buffers(key.len());
                let mut key_plan = self.find_free_databuf(req_key, 0)?;
                key_plan
                    .mutated
                    .write_list_key_payload(key_plan.local_idx, key)
                    .map_err(|_| TurboError::PayloadTooLarge)?;
                let key_pool_hop = self.databuf_slot_to_hop(key_plan.start_slot as u32)?;
                let key_buf_idx = DataBuf::global_buf_idx(key_pool_hop, key_plan.local_idx);

                let mut item_plan = self.find_free_databuf(req_item, key_plan.start_slot)?;
                item_plan
                    .mutated
                    .write_list_val_payload(item_plan.local_idx, item)
                    .map_err(|_| TurboError::PayloadTooLarge)?;
                let item_pool_hop = self.databuf_slot_to_hop(item_plan.start_slot as u32)?;
                let item_buf_idx = DataBuf::global_buf_idx(item_pool_hop, item_plan.local_idx);

                let (arr_idx, block_slot, orig_block, mut mut_block) =
                    self.find_free_adjarr_array(0)?;
                let (_, local_arr) = decompose_relative_array_idx(arr_idx);
                mut_block.array_mut(local_arr).push(item_buf_idx);

                let mut new_bucket = old_bucket;
                new_bucket.set_list(target_entry_idx, hash, arr_idx, key_buf_idx);

                let mut changes: Vec<(usize, Slot, Slot)> = Vec::new();
                for i in 0..BUCKET_SLOTS {
                    let orig_s = old_bucket.as_slots()[i];
                    let new_s = new_bucket.as_slots()[i];
                    if orig_s != new_s {
                        changes.push(((target_bucket_slot as usize) + i, orig_s, new_s));
                    }
                }

                changes.push((block_slot, *orig_block.as_slot(), *mut_block.as_slot()));

                if key_plan.start_slot == item_plan.start_slot {
                    for i in 0..DATABUF_SLOTS {
                        let orig_s = key_plan.orig.as_slots()[i];
                        let new_s = item_plan.mutated.as_slots()[i];
                        if orig_s != new_s {
                            changes.push((key_plan.start_slot + i, orig_s, new_s));
                        }
                    }
                } else {
                    for i in 0..DATABUF_SLOTS {
                        let orig_s = key_plan.orig.as_slots()[i];
                        let new_s = key_plan.mutated.as_slots()[i];
                        if orig_s != new_s {
                            changes.push((key_plan.start_slot + i, orig_s, new_s));
                        }
                    }
                    for i in 0..DATABUF_SLOTS {
                        let orig_s = item_plan.orig.as_slots()[i];
                        let new_s = item_plan.mutated.as_slots()[i];
                        if orig_s != new_s {
                            changes.push((item_plan.start_slot + i, orig_s, new_s));
                        }
                    }
                }

                changes.sort_by_key(|(s, _, _)| *s);
                let mut tx = self.mmap.new_tx();
                for (s, orig_s, new_s) in &changes {
                    if let Err(e) = tx.write(orig_s, new_s, *s) {
                        return Err(TurboError::from(e));
                    }
                }

                match tx.commit() {
                    Ok(ticket) => {
                        self.total_entries.fetch_add(1, Ordering::Relaxed);
                        return Ok(TurboTicket::new(ticket));
                    }
                    Err(_) => {
                        Self::occ_backoff(attempt);
                        continue;
                    }
                }
            }
        }

        Err(TurboError::ResourceBusy("OCC write conflict limit reached"))
    }

    /// Ref: `DESIGN.md` §3.4
    fn find_free_adjarr_array(
        &self,
        _exclude_slot: usize,
    ) -> TurboResult<(u32, usize, AdjArrayBlock, AdjArrayBlock)> {
        let mut curr = ADJ_ARR_HEAD_SLOT as usize;
        let mut last_slot = curr;
        let mut block_hop = 0u32;

        while curr != 0 {
            last_slot = curr;
            let (next, opt_alloc) = self.mmap.read_exact::<AdjArrayBlock, _, _>(curr, |b| {
                let next = b.next_block() as usize;
                let mut mutated = *b;
                match mutated.alloc_array() {
                    Some(local_idx) => (next, Some((local_idx, *b, mutated))),
                    None => (next, None),
                }
            })?;

            if let Some((local_idx, orig, mutated)) = opt_alloc {
                let relative_idx = relative_array_idx(block_hop, local_idx);
                return Ok((relative_idx, curr, orig, mutated));
            }

            curr = next;
            block_hop += 1;
        }

        let new_slot = self.expand_adjarr(last_slot)? as usize;
        let mut block = self.mmap.read_exact::<AdjArrayBlock, _, _>(new_slot, |b| *b)?;
        let orig = block;
        let local_idx = block.alloc_array().ok_or(TurboError::DatabaseFull)?;
        let relative_idx = relative_array_idx(block_hop, local_idx);

        Ok((relative_idx, new_slot, orig, block))
    }

    /// Ref: `DESIGN.md` §7.4
    pub(crate) fn list_pop(&self, key: &[u8]) -> TurboResult<Option<Vec<u8>>> {
        if key.is_empty() || key.len() > MAX_KEY_LEN {
            return Ok(None);
        }

        let hash = hash_key(key);

        for attempt in 0..0x20 {
            let mut bucket_slot = ROOT_BUCKET_SLOT;
            let mut depth = 0u8;

            let probe_res = loop {
                let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;
                let res = bucket.probe(hash, depth, |entry_idx| {
                    self.match_list_key(&bucket.entry(entry_idx), key)
                });

                match res {
                    ProbeResult::Found(entry_idx) => {
                        break Ok((bucket_slot, bucket, entry_idx));
                    }
                    ProbeResult::Vacant(_) => return Ok(None),
                    ProbeResult::Overflow { next_bucket, .. } => {
                        if next_bucket == 0 {
                            return Ok(None);
                        }
                        bucket_slot = next_bucket;
                        depth = depth.wrapping_add(1);
                    }
                }
            };

            let (_target_bucket_slot, old_bucket, target_entry_idx) = match probe_res {
                Ok(res) => res,
                Err(e) => return Err(e),
            };

            let entry = old_bucket.entry(target_entry_idx);
            let mut curr_arr_idx = entry.adjarr_idx();
            let mut prev_info: Option<(u32, usize, AdjArrayBlock, usize)> = None;

            let (prev_opt, last_block_slot, last_orig_block, last_local_arr) = loop {
                let (block_hop, local_arr) = decompose_relative_array_idx(curr_arr_idx);
                let block_slot = self.locate_adjarr_block_slot(block_hop)? as usize;
                let block = self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot, |b| *b)?;

                let next = block.array(local_arr).next_array();
                if next == 0 {
                    break (prev_info, block_slot, block, local_arr);
                }

                prev_info = Some((curr_arr_idx, block_slot, block, local_arr));
                curr_arr_idx = next;
            };

            let mut mutated_last_block = last_orig_block;
            let popped_entry = match mutated_last_block.array_mut(last_local_arr).pop() {
                Some(e) => e,
                None => return Ok(None),
            };

            let (item_pool_hop, item_local_idx) =
                DataBuf::decompose_buf_idx(popped_entry.buf_idx());
            let item_dbuf_slot = self.locate_databuf_slot(item_pool_hop)? as usize;
            let orig_item_dbuf = self.mmap.read_exact::<DataBuf, _, _>(item_dbuf_slot, |d| *d)?;

            let payload_raw = orig_item_dbuf.payload_bytes();
            let start_byte = item_local_idx * 0x10;
            let vlen =
                u16::from_le_bytes([payload_raw[start_byte + 4], payload_raw[start_byte + 5]])
                    as usize;

            let val_bytes = orig_item_dbuf
                .read_list_val_payload(item_local_idx, vlen)
                .map_err(|_| TurboError::CorruptedPayload("List item CRC mismatch"))?
                .to_vec();

            let req_item = DataBuf::required_list_val_buffers(vlen);
            let mut freed_item_dbuf = orig_item_dbuf;
            freed_item_dbuf.free_contiguous(item_local_idx, req_item);

            let mut touched_blocks: Vec<(usize, AdjArrayBlock, AdjArrayBlock)> = Vec::new();

            if mutated_last_block.array(last_local_arr).is_empty() {
                if let Some((_prev_arr, prev_block_slot, prev_orig_block, prev_local_arr)) =
                    prev_opt
                {
                    mutated_last_block.free_array(last_local_arr);

                    let mut mutated_prev_block = prev_orig_block;
                    mutated_prev_block.array_mut(prev_local_arr).set_next_array(0);

                    if prev_block_slot == last_block_slot {
                        mutated_last_block.array_mut(prev_local_arr).set_next_array(0);
                        touched_blocks.push((last_block_slot, last_orig_block, mutated_last_block));
                    } else {
                        touched_blocks.push((last_block_slot, last_orig_block, mutated_last_block));
                        touched_blocks.push((prev_block_slot, prev_orig_block, mutated_prev_block));
                    }
                } else {
                    touched_blocks.push((last_block_slot, last_orig_block, mutated_last_block));
                }
            } else {
                touched_blocks.push((last_block_slot, last_orig_block, mutated_last_block));
            }

            let mut changes: Vec<(usize, Slot, Slot)> = Vec::new();
            for (bs, orig_b, mut_b) in &touched_blocks {
                let orig_s = *orig_b.as_slot();
                let new_s = *mut_b.as_slot();
                if orig_s != new_s {
                    changes.push((*bs, orig_s, new_s));
                }
            }

            for i in 0..DATABUF_SLOTS {
                let orig_s = orig_item_dbuf.as_slots()[i];
                let new_s = freed_item_dbuf.as_slots()[i];
                if orig_s != new_s {
                    changes.push((item_dbuf_slot + i, orig_s, new_s));
                }
            }

            changes.sort_by_key(|(s, _, _)| *s);
            let mut tx = self.mmap.new_tx();
            for (s, orig_s, new_s) in &changes {
                if let Err(e) = tx.write(orig_s, new_s, *s) {
                    return Err(TurboError::from(e));
                }
            }

            match tx.commit() {
                Ok(_) => return Ok(Some(val_bytes)),
                Err(_) => {
                    Self::occ_backoff(attempt);
                    continue;
                }
            }
        }

        Err(TurboError::ResourceBusy("OCC write conflict limit reached"))
    }

    /// Ref: `DESIGN.md` §7.4
    pub(crate) fn list_first(&self, key: &[u8]) -> TurboResult<Option<Vec<u8>>> {
        if key.is_empty() || key.len() > MAX_KEY_LEN {
            return Ok(None);
        }

        let hash = hash_key(key);
        let mut bucket_slot = ROOT_BUCKET_SLOT;
        let mut depth = 0u8;

        loop {
            let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;
            let probe_res = bucket
                .probe(hash, depth, |entry_idx| self.match_list_key(&bucket.entry(entry_idx), key));

            match probe_res {
                ProbeResult::Found(entry_idx) => {
                    let entry = bucket.entry(entry_idx);
                    let (block_hop, local_arr) = decompose_relative_array_idx(entry.adjarr_idx());
                    let block_slot = self.locate_adjarr_block_slot(block_hop)? as usize;
                    let block = self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot, |b| *b)?;

                    let array = block.array(local_arr);
                    let Some(first_entry) = array.entry(0) else {
                        return Ok(None);
                    };
                    if first_entry.is_empty() {
                        return Ok(None);
                    }

                    return self.read_list_item_payload(first_entry.buf_idx()).map(Some);
                }
                ProbeResult::Vacant(_) => return Ok(None),
                ProbeResult::Overflow { next_bucket, .. } => {
                    if next_bucket == 0 {
                        return Ok(None);
                    }
                    bucket_slot = next_bucket;
                    depth = depth.wrapping_add(1);
                }
            }
        }
    }

    /// Ref: `DESIGN.md` §7.4
    pub(crate) fn list_last(&self, key: &[u8]) -> TurboResult<Option<Vec<u8>>> {
        if key.is_empty() || key.len() > MAX_KEY_LEN {
            return Ok(None);
        }

        let hash = hash_key(key);
        let mut bucket_slot = ROOT_BUCKET_SLOT;
        let mut depth = 0u8;

        loop {
            let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;
            let probe_res = bucket
                .probe(hash, depth, |entry_idx| self.match_list_key(&bucket.entry(entry_idx), key));

            match probe_res {
                ProbeResult::Found(entry_idx) => {
                    let entry = bucket.entry(entry_idx);
                    let mut curr_arr_idx = entry.adjarr_idx();
                    let mut last_occupied_buf: Option<u32> = None;

                    loop {
                        let (block_hop, local_arr) = decompose_relative_array_idx(curr_arr_idx);
                        let block_slot = self.locate_adjarr_block_slot(block_hop)? as usize;
                        let block =
                            self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot, |b| *b)?;
                        let array = block.array(local_arr);

                        for i in (0..ADJ_ARRAY_ENTRIES).rev() {
                            if let Some(e) = array.entry(i) {
                                if !e.is_empty() {
                                    last_occupied_buf = Some(e.buf_idx());
                                    break;
                                }
                            }
                        }

                        let next = array.next_array();
                        if next == 0 {
                            break;
                        }
                        curr_arr_idx = next;
                    }

                    return match last_occupied_buf {
                        Some(buf_idx) => self.read_list_item_payload(buf_idx).map(Some),
                        None => Ok(None),
                    };
                }
                ProbeResult::Vacant(_) => return Ok(None),
                ProbeResult::Overflow { next_bucket, .. } => {
                    if next_bucket == 0 {
                        return Ok(None);
                    }
                    bucket_slot = next_bucket;
                    depth = depth.wrapping_add(1);
                }
            }
        }
    }

    /// Ref: `DESIGN.md` §3.3
    fn read_list_item_payload(&self, buf_idx: u32) -> TurboResult<Vec<u8>> {
        let (pool_hop, local_idx) = DataBuf::decompose_buf_idx(buf_idx);
        let dbuf_slot = self.locate_databuf_slot(pool_hop)? as usize;
        let dbuf = self.mmap.read_exact::<DataBuf, _, _>(dbuf_slot, |d| *d)?;

        let raw = dbuf.payload_bytes();
        let start_byte = local_idx * 0x10;
        let vlen = u16::from_le_bytes([raw[start_byte + 4], raw[start_byte + 5]]) as usize;

        let val = dbuf
            .read_list_val_payload(local_idx, vlen)
            .map_err(|_| TurboError::CorruptedPayload("List item CRC mismatch"))?;

        Ok(val.to_vec())
    }

    /// Ref: `DESIGN.md` §7.4
    pub(crate) fn list_get_at(&self, key: &[u8], idx: usize) -> TurboResult<Option<Vec<u8>>> {
        if key.is_empty() || key.len() > MAX_KEY_LEN {
            return Ok(None);
        }

        let hash = hash_key(key);
        let mut bucket_slot = ROOT_BUCKET_SLOT;
        let mut depth = 0u8;

        loop {
            let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;
            let probe_res = bucket
                .probe(hash, depth, |entry_idx| self.match_list_key(&bucket.entry(entry_idx), key));

            match probe_res {
                ProbeResult::Found(entry_idx) => {
                    let entry = bucket.entry(entry_idx);
                    let mut curr_arr_idx = entry.adjarr_idx();
                    let mut current_offset = 0usize;

                    loop {
                        let (block_hop, local_arr) = decompose_relative_array_idx(curr_arr_idx);
                        let block_slot = self.locate_adjarr_block_slot(block_hop)? as usize;
                        let block =
                            self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot, |b| *b)?;
                        let array = block.array(local_arr);

                        for i in 0..ADJ_ARRAY_ENTRIES {
                            if let Some(e) = array.entry(i) {
                                if !e.is_empty() {
                                    if current_offset == idx {
                                        return self.read_list_item_payload(e.buf_idx()).map(Some);
                                    }
                                    current_offset += 1;
                                }
                            }
                        }

                        let next = array.next_array();
                        if next == 0 {
                            break;
                        }
                        curr_arr_idx = next;
                    }

                    return Ok(None);
                }
                ProbeResult::Vacant(_) => return Ok(None),
                ProbeResult::Overflow { next_bucket, .. } => {
                    if next_bucket == 0 {
                        return Ok(None);
                    }
                    bucket_slot = next_bucket;
                    depth = depth.wrapping_add(1);
                }
            }
        }
    }

    /// Ref: `DESIGN.md` §7.4
    pub(crate) fn list_len(&self, key: &[u8]) -> TurboResult<usize> {
        if key.is_empty() || key.len() > MAX_KEY_LEN {
            return Ok(0);
        }

        let hash = hash_key(key);
        let mut bucket_slot = ROOT_BUCKET_SLOT;
        let mut depth = 0u8;

        loop {
            let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;
            let probe_res = bucket
                .probe(hash, depth, |entry_idx| self.match_list_key(&bucket.entry(entry_idx), key));

            match probe_res {
                ProbeResult::Found(entry_idx) => {
                    let entry = bucket.entry(entry_idx);
                    let mut curr_arr_idx = entry.adjarr_idx();
                    let mut count = 0usize;

                    loop {
                        let (block_hop, local_arr) = decompose_relative_array_idx(curr_arr_idx);
                        let block_slot = self.locate_adjarr_block_slot(block_hop)? as usize;
                        let block =
                            self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot, |b| *b)?;
                        let array = block.array(local_arr);

                        count += array.len();
                        let next = array.next_array();
                        if next == 0 {
                            break;
                        }
                        curr_arr_idx = next;
                    }

                    return Ok(count);
                }
                ProbeResult::Vacant(_) => return Ok(0),
                ProbeResult::Overflow { next_bucket, .. } => {
                    if next_bucket == 0 {
                        return Ok(0);
                    }
                    bucket_slot = next_bucket;
                    depth = depth.wrapping_add(1);
                }
            }
        }
    }

    /// Ref: `DESIGN.md` §7.4
    pub(crate) fn list_items(&self, key: &[u8]) -> TurboResult<Vec<Vec<u8>>> {
        let mut items = Vec::new();
        if key.is_empty() || key.len() > MAX_KEY_LEN {
            return Ok(items);
        }

        let hash = hash_key(key);
        let mut bucket_slot = ROOT_BUCKET_SLOT;
        let mut depth = 0u8;

        loop {
            let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;
            let probe_res = bucket
                .probe(hash, depth, |entry_idx| self.match_list_key(&bucket.entry(entry_idx), key));

            match probe_res {
                ProbeResult::Found(entry_idx) => {
                    let entry = bucket.entry(entry_idx);
                    let mut curr_arr_idx = entry.adjarr_idx();

                    loop {
                        let (block_hop, local_arr) = decompose_relative_array_idx(curr_arr_idx);
                        let block_slot = self.locate_adjarr_block_slot(block_hop)? as usize;
                        let block =
                            self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot, |b| *b)?;
                        let array = block.array(local_arr);

                        for i in 0..ADJ_ARRAY_ENTRIES {
                            if let Some(e) = array.entry(i) {
                                if !e.is_empty() {
                                    items.push(self.read_list_item_payload(e.buf_idx())?);
                                }
                            }
                        }

                        let next = array.next_array();
                        if next == 0 {
                            break;
                        }
                        curr_arr_idx = next;
                    }

                    return Ok(items);
                }
                ProbeResult::Vacant(_) => return Ok(items),
                ProbeResult::Overflow { next_bucket, .. } => {
                    if next_bucket == 0 {
                        return Ok(items);
                    }
                    bucket_slot = next_bucket;
                    depth = depth.wrapping_add(1);
                }
            }
        }
    }

    /// Ref: `DESIGN.md` §7.4
    pub(crate) fn list_del(&self, key: &[u8]) -> TurboResult<bool> {
        if key.is_empty() || key.len() > MAX_KEY_LEN {
            return Ok(false);
        }

        let hash = hash_key(key);

        for attempt in 0..0x20 {
            let mut bucket_slot = ROOT_BUCKET_SLOT;
            let mut depth = 0u8;

            let probe_res = loop {
                let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;
                let res = bucket.probe(hash, depth, |entry_idx| {
                    self.match_any_key(&bucket.entry(entry_idx), key)
                });

                match res {
                    ProbeResult::Found(entry_idx) => {
                        break Ok((bucket_slot, bucket, entry_idx));
                    }
                    ProbeResult::Vacant(_) => return Ok(false),
                    ProbeResult::Overflow { next_bucket, .. } => {
                        if next_bucket == 0 {
                            return Ok(false);
                        }
                        bucket_slot = next_bucket;
                        depth = depth.wrapping_add(1);
                    }
                }
            };

            let (target_bucket_slot, old_bucket, target_entry_idx) = match probe_res {
                Ok(res) => res,
                Err(e) => return Err(e),
            };

            let entry = old_bucket.entry(target_entry_idx);
            let mut dbuf_diffs: std::collections::BTreeMap<usize, (DataBuf, DataBuf)> =
                std::collections::BTreeMap::new();
            let mut block_diffs: std::collections::BTreeMap<usize, (AdjArrayBlock, AdjArrayBlock)> =
                std::collections::BTreeMap::new();

            let mut curr_arr_idx = entry.adjarr_idx();
            while curr_arr_idx != 0 {
                let (block_hop, local_arr) = decompose_relative_array_idx(curr_arr_idx);
                let block_slot = self.locate_adjarr_block_slot(block_hop)? as usize;

                let (orig_block, mut mut_block) = match block_diffs.get(&block_slot) {
                    Some((o, m)) => (*o, *m),
                    None => {
                        let b = self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot, |b| *b)?;
                        (b, b)
                    }
                };

                let array = mut_block.array(local_arr);
                for i in 0..ADJ_ARRAY_ENTRIES {
                    if let Some(e) = array.entry(i) {
                        if !e.is_empty() {
                            let (item_pool_hop, item_local_idx) =
                                DataBuf::decompose_buf_idx(e.buf_idx());
                            let item_dbuf_slot = self.locate_databuf_slot(item_pool_hop)? as usize;

                            let (orig_d, mut mut_d) = match dbuf_diffs.get(&item_dbuf_slot) {
                                Some((o, m)) => (*o, *m),
                                None => {
                                    let d = self
                                        .mmap
                                        .read_exact::<DataBuf, _, _>(item_dbuf_slot, |d| *d)?;
                                    (d, d)
                                }
                            };

                            let start_byte = item_local_idx * 0x10;
                            let vlen = u16::from_le_bytes([
                                mut_d.payload_bytes()[start_byte + 4],
                                mut_d.payload_bytes()[start_byte + 5],
                            ]) as usize;
                            let req = DataBuf::required_list_val_buffers(vlen);
                            mut_d.free_contiguous(item_local_idx, req);
                            dbuf_diffs.insert(item_dbuf_slot, (orig_d, mut_d));
                        }
                    }
                }

                let next = mut_block.array(local_arr).next_array();
                mut_block.free_array(local_arr);
                block_diffs.insert(block_slot, (orig_block, mut_block));
                curr_arr_idx = next;
            }

            let req_k = DataBuf::required_list_key_buffers(key.len());
            let (k_pool_hop, k_local_idx) = DataBuf::decompose_buf_idx(entry.buf_idx());
            let k_dbuf_slot = self.locate_databuf_slot(k_pool_hop)? as usize;

            let (orig_kd, mut mut_kd) = match dbuf_diffs.get(&k_dbuf_slot) {
                Some((o, m)) => (*o, *m),
                None => {
                    let d = self.mmap.read_exact::<DataBuf, _, _>(k_dbuf_slot, |d| *d)?;
                    (d, d)
                }
            };
            mut_kd.free_contiguous(k_local_idx, req_k);
            dbuf_diffs.insert(k_dbuf_slot, (orig_kd, mut_kd));

            let mut new_bucket = old_bucket;
            new_bucket.mark_tombstone(target_entry_idx);

            let mut changes: Vec<(usize, Slot, Slot)> = Vec::new();
            for i in 0..BUCKET_SLOTS {
                let orig_s = old_bucket.as_slots()[i];
                let new_s = new_bucket.as_slots()[i];
                if orig_s != new_s {
                    changes.push(((target_bucket_slot as usize) + i, orig_s, new_s));
                }
            }

            for (bs, (orig_b, mut_b)) in &block_diffs {
                let orig_s = *orig_b.as_slot();
                let new_s = *mut_b.as_slot();
                if orig_s != new_s {
                    changes.push((*bs, orig_s, new_s));
                }
            }

            for (ds, (orig_d, mut_d)) in &dbuf_diffs {
                for i in 0..DATABUF_SLOTS {
                    let orig_s = orig_d.as_slots()[i];
                    let new_s = mut_d.as_slots()[i];
                    if orig_s != new_s {
                        changes.push((ds + i, orig_s, new_s));
                    }
                }
            }

            changes.sort_by_key(|(s, _, _)| *s);
            let mut tx = self.mmap.new_tx();
            for (s, orig_s, new_s) in &changes {
                if let Err(e) = tx.write(orig_s, new_s, *s) {
                    return Err(TurboError::from(e));
                }
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

        Err(TurboError::ResourceBusy("OCC write conflict limit reached"))
    }

    /// Ref: `DESIGN.md` §3.1
    #[inline(always)]
    pub(crate) fn total_entries(&self) -> u64 {
        self.total_entries.load(Ordering::Acquire)
    }

    /// Ref: `DESIGN.md` §3.1
    #[inline(always)]
    pub(crate) fn total_pages(&self) -> TurboResult<u32> {
        let meta = self.mmap.read_exact::<Metadata, _, _>(0, |m| *m)?;
        Ok(meta.total_pages)
    }

    /// Ref: `DESIGN.md` §9.1
    #[inline(always)]
    pub(crate) fn remaining_pages(&self) -> TurboResult<u32> {
        let page_size = self.sys_page_size.as_usize();
        let curr_len = self.mmap.length();
        let remaining_bytes = MAX_DB_LEN.saturating_sub(curr_len);

        Ok((remaining_bytes / page_size) as u32)
    }

    /// Ref: `DESIGN.md` §9.1
    #[inline(always)]
    pub(crate) fn remaining_bytes(&self) -> TurboResult<u64> {
        let curr_len = self.mmap.length();
        Ok((MAX_DB_LEN.saturating_sub(curr_len)) as u64)
    }

    /// Ref: `DESIGN.md` §3.3
    #[inline(always)]
    pub(crate) fn active_dbuf_hint(&self) -> u32 {
        self.active_dbuf_hint.load(Ordering::Relaxed)
    }

    /// Ref: `DESIGN.md` §8.1
    pub(crate) fn sync(&self) -> TurboResult<()> {
        let entries = self.total_entries.load(Ordering::Acquire);
        self.mmap.write_exact::<Metadata, _, _>(0, |m| {
            m.total_entries = entries;
        })?;

        self.mmap.sync()?;
        Ok(())
    }

    /// Ref: `DESIGN.md` §8.1
    pub(crate) fn delete(self) -> TurboResult<()> {
        self.mmap.delete()?;
        Ok(())
    }
}
