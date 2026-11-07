use crate::adjarr::{AdjArrayBlock, decompose_relative_array_idx, relative_array_idx};
use crate::bucket::{Bucket, BucketEntry, ProbeResult};
use crate::cfg::{EvictionMode, TurboCfg};
use crate::common::{
    ADJ_ARR_HEAD_SLOT, ADJ_ARR_SLOTS, ALLOC_SLOT_QUANTUM, BUCKET_ENTRIES, BUCKET_SLOTS,
    DATABUF_HEAD_SLOT, DB_NAME_TAG, DBUF_BUF_COUNT, DBUF_SLOTS, EXTRA_NODE_START_SLOT,
    INITIAL_BUCKET_COUNT, INITIAL_BUCKET_START_SLOT, INITIAL_DATABUF_POOLS, INITIAL_EXTRA_NODES,
    INITIAL_FREE_SLOT_MARKER, LAYOUT_VERSION, MAGIC, MAX_DB_LEN, MAX_KEY_LEN, MAX_TRIE_DEPTH,
    MAX_VAL_LEN, MODULE_ID, NODE_CHILDREN, OCC_MAX_RETRIES, ROOT_NODE_SLOT, SLOT_SIZE, Slot,
};
use crate::dbuf::{
    DBuf, mark_spanned_buffers, read_kv_payload_spanned, read_list_val_payload_spanned,
    unmark_spanned_buffers, verify_key_spanned, verify_list_key_spanned, write_kv_payload_spanned,
    write_list_key_payload_spanned, write_list_val_payload_spanned,
};
use crate::error::{TurboError, TurboResult};
use crate::hash::{hash_kv_key, hash_list_key, trie_child_idx};
use crate::meta::Metadata;
use crate::node::Node;
use crate::ticket::TurboTicket;
use frozen_core::memmap::{MemMap, MemMapCfg, PageSize};
use std::sync::{
    Mutex,
    atomic::{AtomicPtr, AtomicU32, AtomicU64, Ordering},
};

pub(crate) struct DBufSlotCache {
    chunks: [AtomicPtr<[AtomicU32; 0x1000]>; 0x0400],
}

impl std::fmt::Debug for DBufSlotCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DBufSlotCache").finish()
    }
}

unsafe impl Send for DBufSlotCache {}
unsafe impl Sync for DBufSlotCache {}

impl DBufSlotCache {
    pub(crate) const CHUNK_SHIFT: usize = 0x0C;
    pub(crate) const CHUNK_MASK: usize = 0x0FFF;
    pub(crate) const CHUNK_SIZE: usize = 0x1000;
    pub(crate) const MAX_CHUNKS: usize = 0x0400;

    pub(crate) fn new() -> Self {
        Self { chunks: std::array::from_fn(|_| AtomicPtr::new(std::ptr::null_mut())) }
    }

    pub(crate) fn get(&self, pool_hop: u32) -> Option<u32> {
        let chunk_idx = (pool_hop as usize) >> Self::CHUNK_SHIFT;
        let offset = (pool_hop as usize) & Self::CHUNK_MASK;
        if chunk_idx >= Self::MAX_CHUNKS {
            return None;
        }

        let chunk_ptr = self.chunks[chunk_idx].load(Ordering::Acquire);
        if chunk_ptr.is_null() {
            return None;
        }

        let slot = unsafe { (*chunk_ptr)[offset].load(Ordering::Acquire) };
        if slot == 0 { None } else { Some(slot) }
    }

    pub(crate) fn push(&self, pool_hop: u32, slot: u32) {
        let chunk_idx = (pool_hop as usize) >> Self::CHUNK_SHIFT;
        let offset = (pool_hop as usize) & Self::CHUNK_MASK;

        if chunk_idx >= Self::MAX_CHUNKS {
            return;
        }

        let mut chunk_ptr = self.chunks[chunk_idx].load(Ordering::Acquire);
        if chunk_ptr.is_null() {
            let mut v = Vec::with_capacity(Self::CHUNK_SIZE);
            for _ in 0..Self::CHUNK_SIZE {
                v.push(AtomicU32::new(0));
            }

            let boxed: Box<[AtomicU32]> = v.into_boxed_slice();
            let boxed_arr: Box<[AtomicU32; 0x1000]> = match boxed.try_into() {
                Ok(b) => b,
                Err(_) => unreachable!(),
            };

            let new_ptr = Box::into_raw(boxed_arr);
            match self.chunks[chunk_idx].compare_exchange(
                std::ptr::null_mut(),
                new_ptr,
                Ordering::Release,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    chunk_ptr = new_ptr;
                }
                Err(existing) => {
                    unsafe { drop(Box::from_raw(new_ptr)) };

                    chunk_ptr = existing;
                }
            }
        }

        unsafe {
            (*chunk_ptr)[offset].store(slot, Ordering::Release);
        }
    }

    pub(crate) fn memory_bytes(&self) -> usize {
        let chunk_size = core::mem::size_of::<[AtomicU32; Self::CHUNK_SIZE]>();
        let allocated_chunks =
            self.chunks.iter().filter(|chunk| !chunk.load(Ordering::Acquire).is_null()).count();
        core::mem::size_of::<Self>().saturating_add(allocated_chunks.saturating_mul(chunk_size))
    }

    pub(crate) fn clear(&self) {
        for chunk in &self.chunks {
            let ptr = chunk.swap(std::ptr::null_mut(), Ordering::AcqRel);
            if !ptr.is_null() {
                unsafe { drop(Box::from_raw(ptr)) };
            }
        }
    }
}

impl Drop for DBufSlotCache {
    fn drop(&mut self) {
        for chunk in &self.chunks {
            let ptr = chunk.load(Ordering::Relaxed);
            if !ptr.is_null() {
                unsafe { drop(Box::from_raw(ptr)) };
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DbufAllocPlan {
    start_slot: usize,
    pool_hop: u32,
    local_idx: usize,
    orig: DBuf,
    mutated: DBuf,
}

#[derive(Clone, Debug)]
pub(crate) struct DbufSpanPlan {
    pub(crate) start_global_buf: u32,
    pub(crate) pools: Vec<(usize, DBuf, DBuf)>,
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
    dbuf_slot_cache: Option<DBufSlotCache>,
}

impl Engine {
    /// Ref: `DESIGN.md` §8.1
    pub(crate) fn create(cfg: TurboCfg) -> TurboResult<Self> {
        let page_size = MemMap::<Slot>::page_size()?;
        let initial_pages = cfg.resolve_initial_pages(page_size)?;

        let dir_path = cfg.resolve_dir();
        let memmap_cfg = MemMapCfg {
            module_id: MODULE_ID,
            path: dir_path,
            name: DB_NAME_TAG,
            version: LAYOUT_VERSION,
            initial_pages,
            max_len: MAX_DB_LEN,
            sync_interval: cfg.sync_interval(),
            memory_priority_hint: true,
            sequential_access_hint: cfg.lookup_cache,
        };

        let mmap = MemMap::<Slot>::new(memmap_cfg)?;
        let initial_slots = (initial_pages * page_size.as_usize()) / SLOT_SIZE;

        let engine = Self {
            mmap,
            growth_mutex: Mutex::new(()),
            total_slots: AtomicU32::new(initial_slots as u32),
            total_entries: AtomicU64::new(0),
            free_slot_marker: AtomicU32::new(INITIAL_FREE_SLOT_MARKER),
            active_dbuf_hint: AtomicU32::new(0),
            sys_page_size: page_size,
            dbuf_slot_cache: if cfg.lookup_cache { Some(DBufSlotCache::new()) } else { None },
        };

        engine.format_initial_topography(LAYOUT_VERSION, initial_pages as u32)?;
        engine.sync()?;

        Ok(engine)
    }

    /// Ref: `DESIGN.md` §8.1
    pub(crate) fn open_or_create(cfg: TurboCfg) -> TurboResult<Self> {
        let memmap_cfg = MemMapCfg {
            module_id: MODULE_ID,
            path: cfg.resolve_dir(),
            name: DB_NAME_TAG,
            version: LAYOUT_VERSION,
            initial_pages: 1,
            max_len: MAX_DB_LEN,
            sync_interval: cfg.sync_interval(),
            memory_priority_hint: true,
            sequential_access_hint: cfg.lookup_cache,
        };

        if memmap_cfg.versioned_path().exists() { Self::open(cfg) } else { Self::create(cfg) }
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
            sync_interval: cfg.sync_interval(),
            memory_priority_hint: true,
            sequential_access_hint: cfg.lookup_cache,
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

        let raw_slots = file_len / SLOT_SIZE;
        let expected_slots =
            if raw_slots > (0xFFFF_FFFFusize) { 0xFFFF_FFFFu32 } else { raw_slots as u32 };

        if meta.total_slots != expected_slots {
            return Err(TurboError::CorruptedHeader("metadata total_slots divergence"));
        }

        let expected_pages = (file_len / sys_page_size.as_usize()) as u32;
        if meta.total_pages != expected_pages {
            return Err(TurboError::CorruptedHeader("metadata total_pages divergence"));
        }

        let root_flag = mmap.read_exact::<Slot, _, _>(ROOT_NODE_SLOT as usize, |s| s.flag())?;
        if (root_flag & !0x7FFF) != 0 {
            return Err(TurboError::InvalidRootNode);
        }

        let engine = Self {
            mmap,
            growth_mutex: Mutex::new(()),
            total_slots: AtomicU32::new(meta.total_slots),
            total_entries: AtomicU64::new(meta.total_entries),
            free_slot_marker: AtomicU32::new(meta.free_slot_marker),
            active_dbuf_hint: AtomicU32::new(0),
            sys_page_size,
            dbuf_slot_cache: if cfg.lookup_cache { Some(DBufSlotCache::new()) } else { None },
        };

        Ok(engine)
    }

    /// Ref: `DESIGN.md` §2
    fn format_initial_topography(&self, version: u32, initial_pages: u32) -> TurboResult<()> {
        let meta = Metadata::new(self.sys_page_size.as_u32(), version, initial_pages);
        let _ = self.mmap.write_exact::<Metadata, _, _>(0, |m| *m = meta)?;

        let block_head = AdjArrayBlock::new();
        let _ = self
            .mmap
            .write_exact::<AdjArrayBlock, _, _>(ADJ_ARR_HEAD_SLOT as usize, |b| *b = block_head)?;

        for i in 0..INITIAL_DATABUF_POOLS {
            let pool_slot = DATABUF_HEAD_SLOT + (i as u32) * (DBUF_SLOTS as u32);
            let next_slot =
                if i + 1 < INITIAL_DATABUF_POOLS { pool_slot + (DBUF_SLOTS as u32) } else { 0 };

            let mut dbuf = DBuf::new();
            dbuf.set_next_dbuf_index(next_slot);

            let _ = self.mmap.write_exact::<DBuf, _, _>(pool_slot as usize, |d| *d = dbuf)?;
        }

        let mut children = [0u32; NODE_CHILDREN];
        for (i, child) in children.iter_mut().enumerate() {
            *child = INITIAL_BUCKET_START_SLOT + (i as u32) * (BUCKET_SLOTS as u32);
        }

        let root_node = Node::with_children(children);
        let _ = self.mmap.write_exact::<Node, _, _>(ROOT_NODE_SLOT as usize, |n| *n = root_node)?;

        for s in 0..INITIAL_BUCKET_COUNT {
            let bucket_slot = INITIAL_BUCKET_START_SLOT + (s as u32) * (BUCKET_SLOTS as u32);
            let bucket = Bucket::new();

            let _ = self.mmap.write_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b = bucket)?;
        }

        for i in 0..INITIAL_EXTRA_NODES {
            let spare_slot = EXTRA_NODE_START_SLOT + (i as u32);
            let spare_node = Node::new();

            let _ =
                self.mmap.write_exact::<Node, _, _>(spare_slot as usize, |n| *n = spare_node)?;
        }

        Ok(())
    }

    fn alloc_uniform_slots_locked(&self, count: u32) -> TurboResult<u32> {
        let current_free = self.free_slot_marker.load(Ordering::Acquire);
        let total = self.total_slots.load(Ordering::Acquire);

        let next_free = match current_free.checked_add(count) {
            Some(next) => next,
            None => return Err(TurboError::DatabaseFull),
        };

        if next_free > total {
            self.grow_backing_file(next_free - total)?;
        }

        let new_total = self.total_slots.load(Ordering::Acquire);
        if next_free > new_total {
            return Err(TurboError::DatabaseFull);
        }

        let allocated = current_free;
        self.free_slot_marker.store(next_free, Ordering::Release);
        let _ = self.mmap.write_exact::<Metadata, _, _>(0, |m| {
            m.free_slot_marker = next_free;
        })?;

        Ok(allocated)
    }

    /// Ref: `DESIGN.md` §5.4
    fn grow_backing_file(&self, needed_slots: u32) -> TurboResult<()> {
        let curr_len = self.mmap.length();
        let page_bytes = self.sys_page_size.as_usize();

        let quantum_pages = match self.sys_page_size {
            PageSize::K4 => 4,
            PageSize::K16 | PageSize::K64 => 1,
        };

        let needed_bytes = match (needed_slots as usize).checked_mul(SLOT_SIZE) {
            Some(b) => b,
            None => return Err(TurboError::DatabaseFull),
        };
        let needed_pages = needed_bytes.div_ceil(page_bytes);
        let growth_pages = needed_pages.div_ceil(quantum_pages) * quantum_pages;
        let added_bytes = match growth_pages.checked_mul(page_bytes) {
            Some(b) => b,
            None => return Err(TurboError::DatabaseFull),
        };

        if curr_len.saturating_add(added_bytes) > MAX_DB_LEN {
            return Err(TurboError::DatabaseFull);
        }

        self.mmap.grow(growth_pages)?;

        let mut new_total_slots = 0u32;
        let _ = self.mmap.write_exact::<Metadata, _, _>(0, |m| {
            m.record_growth(growth_pages as u32);
            new_total_slots = m.total_slots;
        })?;

        self.total_slots.store(new_total_slots, Ordering::Release);
        Ok(())
    }

    fn expand_databuf_locked(&self, tail_slot: usize, min_pools: usize) -> TurboResult<u32> {
        let pool_count = std::cmp::max(4, 4 * min_pools);
        let needed_slots = (pool_count * DBUF_SLOTS) as u32;

        let new_slot = self.alloc_uniform_slots_locked(needed_slots)?;
        for i in 0..pool_count {
            let s = (new_slot as usize) + i * DBUF_SLOTS;
            let next =
                if i + 1 < pool_count { new_slot + ((i + 1) * DBUF_SLOTS) as u32 } else { 0 };

            let mut dbuf = DBuf::new();
            dbuf.set_next_dbuf_index(next);

            let _ = self.mmap.write_exact::<DBuf, _, _>(s, |d| *d = dbuf)?;
        }

        let _ = self.mmap.write_exact::<DBuf, _, _>(tail_slot, |d| {
            d.set_next_dbuf_index(new_slot);
        })?;

        let tail_hop = self.databuf_slot_to_hop(tail_slot as u32)?;
        if let Some(ref cache) = self.dbuf_slot_cache {
            for i in 0..pool_count {
                let s = new_slot + (i * DBUF_SLOTS) as u32;
                let hop = tail_hop + 1 + (i as u32);

                cache.push(hop, s);
            }
        }

        Ok(new_slot)
    }

    /// Ref: `DESIGN.md` §5.3
    fn expand_adjarr_locked(&self, tail_slot: usize) -> TurboResult<u32> {
        let new_slot = self.alloc_uniform_slots_locked(ALLOC_SLOT_QUANTUM)?;
        let block_count = (ALLOC_SLOT_QUANTUM as usize) / ADJ_ARR_SLOTS;

        for i in 0..block_count {
            let s = (new_slot as usize) + i * ADJ_ARR_SLOTS;
            let next =
                if i + 1 < block_count { new_slot + ((i + 1) * ADJ_ARR_SLOTS) as u32 } else { 0 };

            let mut block = AdjArrayBlock::new();
            block.set_next_block(next);

            let _ = self.mmap.write_exact::<AdjArrayBlock, _, _>(s, |b| *b = block)?;
        }

        let _ = self.mmap.write_exact::<AdjArrayBlock, _, _>(tail_slot, |b| {
            b.set_next_block(new_slot);
        })?;

        Ok(new_slot)
    }

    /// Ref: `DESIGN.md` §3.3
    pub(crate) fn locate_databuf_slot(&self, pool_hop: u32) -> TurboResult<u32> {
        if pool_hop == 0 {
            return Ok(DATABUF_HEAD_SLOT);
        }

        if let Some(ref cache) = self.dbuf_slot_cache
            && let Some(slot) = cache.get(pool_hop)
        {
            return Ok(slot);
        }

        let mut start_hop = 0u32;
        let mut curr = DATABUF_HEAD_SLOT;

        if let Some(ref cache) = self.dbuf_slot_cache {
            for h in (1..pool_hop).rev() {
                if let Some(slot) = cache.get(h) {
                    start_hop = h;
                    curr = slot;
                    break;
                }
            }
        }

        for h in (start_hop + 1)..=pool_hop {
            let next =
                self.mmap.read_exact::<DBuf, _, _>(curr as usize, |d| d.next_dbuf_index())?;

            if next == 0 {
                return Err(TurboError::CorruptedHeader("broken databuf chain"));
            }

            curr = next;
            if let Some(ref cache) = self.dbuf_slot_cache {
                cache.push(h, curr);
            }
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

    #[inline(always)]
    fn read_list_adjarr_idx(&self, key_buf_idx: u32) -> TurboResult<u32> {
        let (pool_hop, local_idx) = DBuf::decompose_buf_idx(key_buf_idx);

        let dbuf_slot = self.locate_databuf_slot(pool_hop)? as usize;
        let adjarr = self.mmap.read_exact::<DBuf, _, _>(dbuf_slot, |d| {
            d.read_header_vlen_or_adjarr_index(local_idx)
                .map_err(|_| TurboError::CorruptedPayload("invalid databuf index"))
        })??;

        Ok(adjarr)
    }

    /// Ref: `DESIGN.md` §3.3
    pub(crate) fn find_free_databuf(
        &self,
        needed: usize,
        exclude_slots: &[usize],
    ) -> TurboResult<DbufAllocPlan> {
        let hint_hop = self.active_dbuf_hint.load(Ordering::Relaxed);
        let start_slot = self.locate_databuf_slot(hint_hop).unwrap_or(DATABUF_HEAD_SLOT) as usize;
        let start_hop = if start_slot == (DATABUF_HEAD_SLOT as usize) { 0 } else { hint_hop };

        let mut curr = start_slot;
        let mut curr_hop = start_hop;
        let mut tail_slot = start_slot;
        let mut tail_hop = start_hop;
        let mut wrapped = false;

        loop {
            if exclude_slots.contains(&curr) {
                let next =
                    self.mmap.read_exact::<DBuf, _, _>(curr, |d| d.next_dbuf_index() as usize)?;

                if next != 0 {
                    curr = next;
                    curr_hop += 1;
                } else {
                    tail_slot = curr;
                    tail_hop = curr_hop;

                    if wrapped || start_slot == (DATABUF_HEAD_SLOT as usize) {
                        break;
                    }

                    wrapped = true;
                    curr = DATABUF_HEAD_SLOT as usize;
                    curr_hop = 0;
                }
            } else {
                let (next, alloc_opt) = self.mmap.read_exact::<DBuf, _, _>(curr, |d| {
                    let next = d.next_dbuf_index() as usize;
                    let mut mutated = *d;

                    match mutated.alloc_contiguous(needed) {
                        Some(local_idx) => (next, Some((local_idx, *d, mutated))),
                        None => (next, None),
                    }
                })?;

                if let Some((local_idx, orig, mutated)) = alloc_opt {
                    if wrapped {
                        self.active_dbuf_hint.store(curr_hop, Ordering::Relaxed);
                    }

                    return Ok(DbufAllocPlan {
                        start_slot: curr,
                        pool_hop: curr_hop,
                        local_idx,
                        orig,
                        mutated,
                    });
                }

                if next != 0 {
                    if !wrapped {
                        self.active_dbuf_hint.store(curr_hop + 1, Ordering::Relaxed);
                    }

                    curr = next;
                    curr_hop += 1;
                } else {
                    tail_slot = curr;
                    tail_hop = curr_hop;

                    if wrapped || start_slot == (DATABUF_HEAD_SLOT as usize) {
                        break;
                    }

                    wrapped = true;
                    curr = DATABUF_HEAD_SLOT as usize;
                    curr_hop = 0;
                }
            }

            if wrapped && curr == start_slot {
                break;
            }
        }

        let _guard = self.growth_mutex.lock().unwrap_or_else(|e| e.into_inner());

        let mut actual_tail = tail_slot;
        let mut actual_tail_hop = tail_hop;
        loop {
            let next = self
                .mmap
                .read_exact::<DBuf, _, _>(actual_tail, |d| d.next_dbuf_index() as usize)?;

            if next == 0 {
                let _ = self.expand_databuf_locked(actual_tail, 1)?;
                continue;
            }

            actual_tail = next;
            actual_tail_hop += 1;

            if !exclude_slots.contains(&actual_tail) {
                let (_, alloc_opt) = self.mmap.read_exact::<DBuf, _, _>(actual_tail, |d| {
                    let next = d.next_dbuf_index() as usize;
                    let mut mutated = *d;

                    match mutated.alloc_contiguous(needed) {
                        Some(local_idx) => (next, Some((local_idx, *d, mutated))),
                        None => (next, None),
                    }
                })?;

                if let Some((local_idx, orig, mutated)) = alloc_opt {
                    self.active_dbuf_hint.store(actual_tail_hop, Ordering::Relaxed);

                    return Ok(DbufAllocPlan {
                        start_slot: actual_tail,
                        pool_hop: actual_tail_hop,
                        local_idx,
                        orig,
                        mutated,
                    });
                }
            }
        }
    }

    pub(crate) fn find_free_databuf_span(
        &self,
        needed: usize,
        exclude_slots: &[usize],
    ) -> TurboResult<DbufSpanPlan> {
        if needed <= DBUF_BUF_COUNT {
            let plan = self.find_free_databuf(needed, exclude_slots)?;
            let start_global_buf = DBuf::global_buf_idx(plan.pool_hop, plan.local_idx);

            return Ok(DbufSpanPlan {
                start_global_buf,
                pools: vec![(plan.start_slot, plan.orig, plan.mutated)],
            });
        }

        let needed_pools = needed.div_ceil(DBUF_BUF_COUNT);
        let _guard = self.growth_mutex.lock().unwrap_or_else(|e| e.into_inner());

        let mut curr_hop = 0u32;
        let mut curr_slot = DATABUF_HEAD_SLOT as usize;

        loop {
            let next =
                self.mmap.read_exact::<DBuf, _, _>(curr_slot, |d| d.next_dbuf_index() as usize)?;
            if next == 0 {
                break;
            }

            curr_slot = next;
            curr_hop += 1;
        }

        let new_start_hop = curr_hop + 1;
        let start_global_buf = new_start_hop * (DBUF_BUF_COUNT as u32);
        let new_first_slot = self.expand_databuf_locked(curr_slot, needed_pools)?;

        let mut remaining = needed;
        let mut p_slot = new_first_slot as usize;
        let mut pools = Vec::with_capacity(needed_pools);

        for i in 0..needed_pools {
            let orig = self.mmap.read_exact::<DBuf, _, _>(p_slot, |d| *d)?;
            let mut mutated = orig;

            let count = std::cmp::min(DBUF_BUF_COUNT, remaining);
            mark_spanned_buffers(&mut mutated, 0, count);

            pools.push((p_slot, orig, mutated));
            remaining -= count;

            if i + 1 < needed_pools {
                p_slot = mutated.next_dbuf_index() as usize;
            }
        }

        self.active_dbuf_hint.store(new_start_hop + (needed_pools as u32), Ordering::Relaxed);
        Ok(DbufSpanPlan { start_global_buf, pools })
    }

    /// Ref: `DESIGN.md` §3.3
    pub(crate) fn databuf_slot_to_hop(&self, slot: u32) -> TurboResult<u32> {
        let mut curr = DATABUF_HEAD_SLOT;
        let mut hop = 0u32;

        while curr != 0 {
            if curr == slot {
                return Ok(hop);
            }

            curr = self.locate_databuf_slot(hop + 1).unwrap_or(0);
            hop += 1;
        }

        Err(TurboError::CorruptedHeader("databuf slot not found in allocation chain"))
    }

    #[inline(always)]
    fn fast_jitter(attempt: u32) -> u64 {
        thread_local! {
            static SEED: std::cell::Cell<u64> = const {
                std::cell::Cell::new(0x8888_4444_2222_1111)
            };
        }

        SEED.with(|s| {
            let mut x = s.get().wrapping_add(attempt as u64);
            x ^= x << 0x0D;
            x ^= x >> 0x07;
            x ^= x << 0x11;

            s.set(x);
            0x4B + (x % 0x33)
        })
    }

    /// Ref: `DESIGN.md` §6
    fn occ_backoff(attempt: u32) {
        if attempt < 0x04 {
            let spins = 0x08u32 << attempt;
            for _ in 0..spins {
                std::hint::spin_loop();
            }

            return;
        }

        if attempt < 0x08 {
            std::thread::yield_now();
            return;
        }

        let base_us = 0x14u64.saturating_mul(1u64 << (attempt - 0x08).min(0x05)).min(0x3E8);
        let wait_us = (base_us * Self::fast_jitter(attempt)) / 0x64;

        std::thread::sleep(std::time::Duration::from_micros(wait_us.max(0x01)));
    }

    /// Traverse trie to leaf bucket
    fn traverse_to_leaf(&self, hash: u32) -> TurboResult<(u32, usize, u32, usize)> {
        let mut curr_slot = ROOT_NODE_SLOT;
        let mut depth = 0usize;

        loop {
            let node = self.mmap.read_exact::<Node, _, _>(curr_slot as usize, |n| *n)?;
            let child_idx = trie_child_idx(hash, depth);
            let target_slot = node.bucket_indices[child_idx];

            if node.is_child_node(child_idx) {
                curr_slot = target_slot;
                depth += 1;
            } else {
                return Ok((curr_slot, child_idx, target_slot, depth));
            }
        }
    }

    fn split_and_rehash(
        &self,
        parent_slot: u32,
        parent_child_idx: usize,
        bucket_slot: u32,
        depth: usize,
        all_entries: &[BucketEntry],
    ) -> TurboResult<(u32, Bucket, Node)> {
        if depth >= MAX_TRIE_DEPTH {
            return Err(TurboError::CollisionLimit);
        }

        let (new_node_slot, new_buckets_start) = {
            let _guard = self.growth_mutex.lock().unwrap_or_else(|e| e.into_inner());

            let mut popped = None;
            let _ = self.mmap.write_exact::<Metadata, _, _>(0, |m| {
                popped = m.pop_extra_node();
            })?;

            let node_slot = if let Some(slot) = popped {
                slot
            } else {
                let allocated = self.alloc_uniform_slots_locked(ALLOC_SLOT_QUANTUM)?;
                let _ = self.mmap.write_exact::<Metadata, _, _>(0, |m| {
                    let _ = m.push_extra_node(allocated + 1);
                    let _ = m.push_extra_node(allocated + 2);
                    let _ = m.push_extra_node(allocated + 3);
                })?;
                allocated
            };

            let buckets_start = self.alloc_uniform_slots_locked((0x0E * BUCKET_SLOTS) as u32)?;
            (node_slot, buckets_start)
        };

        let mut groups: [Vec<BucketEntry>; NODE_CHILDREN] = Default::default();
        for e in all_entries {
            let c_idx = trie_child_idx(e.hash(), depth + 1);
            groups[c_idx].push(*e);
        }

        for g in &groups {
            if g.len() > BUCKET_ENTRIES {
                return Err(TurboError::CollisionLimit);
            }
        }

        let mut compacted_bucket = Bucket::new();
        for (idx, e) in groups[0].iter().enumerate() {
            compacted_bucket.set_entry(idx, *e);
        }

        let mut child_slots = [0u32; NODE_CHILDREN];
        child_slots[0] = bucket_slot;

        for i in 1..NODE_CHILDREN {
            let child_slot = new_buckets_start + (((i - 1) * BUCKET_SLOTS) as u32);
            child_slots[i] = child_slot;

            let mut child_b = Bucket::new();
            for (idx, e) in groups[i].iter().enumerate() {
                child_b.set_entry(idx, *e);
            }

            let _ = self.mmap.write_exact::<Bucket, _, _>(child_slot as usize, |b| *b = child_b)?;
        }

        let new_node = Node::with_children(child_slots);
        let _ = self.mmap.write_exact::<Node, _, _>(new_node_slot as usize, |n| *n = new_node)?;

        let orig_parent = self.mmap.read_exact::<Node, _, _>(parent_slot as usize, |n| *n)?;
        let mut mutated_parent = orig_parent;
        mutated_parent.bucket_indices[parent_child_idx] = new_node_slot;
        mutated_parent.set_child_node(parent_child_idx);

        Ok((new_node_slot, compacted_bucket, mutated_parent))
    }

    /// Ref: `DESIGN.md` §7.1
    pub(crate) fn get(&self, key: &[u8]) -> TurboResult<Option<Vec<u8>>> {
        if key.is_empty() || key.len() > MAX_KEY_LEN {
            return Ok(None);
        }

        let hash = hash_kv_key(key);
        let (_, _, bucket_slot, _depth) = self.traverse_to_leaf(hash)?;
        let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;

        let matched_idx =
            bucket.probe_match(hash, |entry_idx| self.match_kv_key(&bucket.entry(entry_idx), key));

        match matched_idx {
            Some(entry_idx) => {
                let entry = bucket.entry(entry_idx);
                let (pool_hop, local_idx) = DBuf::decompose_buf_idx(entry.buf_idx());
                let dbuf_slot = self.locate_databuf_slot(pool_hop)?;
                let dbuf = self.mmap.read_exact::<DBuf, _, _>(dbuf_slot as usize, |d| *d)?;

                let klen = dbuf
                    .read_header_klen(local_idx)
                    .map_err(|_| TurboError::CorruptedPayload("invalid databuf index"))?;
                let vlen = dbuf
                    .read_header_vlen_or_adjarr_index(local_idx)
                    .map_err(|_| TurboError::CorruptedPayload("invalid databuf index"))?
                    as usize;
                let req = DBuf::required_kv_buffers(klen, vlen);

                if req <= DBUF_BUF_COUNT && local_idx + req <= DBUF_BUF_COUNT {
                    let (_, val) = dbuf
                        .read_kv_payload(local_idx, klen, vlen)
                        .map_err(|_| TurboError::CorruptedPayload("KV CRC32 mismatch"))?;

                    Ok(Some(val.to_vec()))
                } else {
                    let start_pool = pool_hop;
                    let end_pool = (entry.buf_idx() + (req as u32) - 1) / (DBUF_BUF_COUNT as u32);

                    let mut dbufs = Vec::with_capacity((end_pool - start_pool + 1) as usize);
                    dbufs.push(dbuf);
                    for p in (start_pool + 1)..=end_pool {
                        let slot = self.locate_databuf_slot(p)?;
                        let d = self.mmap.read_exact::<DBuf, _, _>(slot as usize, |d| *d)?;
                        dbufs.push(d);
                    }

                    let (_, val) = read_kv_payload_spanned(&dbufs, entry.buf_idx(), klen, vlen)
                        .map_err(|_| TurboError::CorruptedPayload("KV CRC32 mismatch"))?;

                    Ok(Some(val))
                }
            }
            None => Ok(None),
        }
    }

    pub(crate) fn contains_key(&self, key: &[u8]) -> TurboResult<bool> {
        if key.is_empty() || key.len() > MAX_KEY_LEN {
            return Ok(false);
        }

        let hash = hash_kv_key(key);
        let (_, _, bucket_slot, _depth) = self.traverse_to_leaf(hash)?;
        let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;

        let matched_idx =
            bucket.probe_match(hash, |entry_idx| self.match_kv_key(&bucket.entry(entry_idx), key));

        Ok(matched_idx.is_some())
    }

    fn match_kv_key(&self, entry: &BucketEntry, key: &[u8]) -> bool {
        if entry.is_list() {
            return false;
        }

        let (pool_hop, local_idx) = DBuf::decompose_buf_idx(entry.buf_idx());
        let req_key = DBuf::required_kv_buffers(key.len(), 0);

        if local_idx + req_key <= DBUF_BUF_COUNT {
            let Ok(dbuf_slot) = self.locate_databuf_slot(pool_hop) else {
                return false;
            };

            let Ok(matches) = self
                .mmap
                .read_exact::<DBuf, _, _>(dbuf_slot as usize, |d| d.verify_key(local_idx, key))
            else {
                return false;
            };

            matches
        } else {
            let start_pool = pool_hop;
            let end_pool = (entry.buf_idx() + (req_key as u32) - 1) / (DBUF_BUF_COUNT as u32);

            let mut dbufs = Vec::with_capacity((end_pool - start_pool + 1) as usize);
            for p in start_pool..=end_pool {
                let Ok(slot) = self.locate_databuf_slot(p) else {
                    return false;
                };

                let Ok(dbuf) = self.mmap.read_exact::<DBuf, _, _>(slot as usize, |d| *d) else {
                    return false;
                };

                dbufs.push(dbuf);
            }

            verify_key_spanned(&dbufs, entry.buf_idx(), key)
        }
    }

    /// Ref: `DESIGN.md` §7.2
    pub(crate) fn set(&self, key: &[u8], val: &[u8]) -> TurboResult<TurboTicket> {
        if key.is_empty() || key.len() > MAX_KEY_LEN || val.len() > MAX_VAL_LEN {
            return Err(TurboError::PayloadTooLarge);
        }

        let req = DBuf::required_kv_buffers(key.len(), val.len());
        let hash = hash_kv_key(key);

        for attempt in 0..OCC_MAX_RETRIES {
            let (parent_slot, parent_child_idx, bucket_slot, depth) =
                self.traverse_to_leaf(hash)?;

            let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;
            let probe_res =
                bucket.probe(hash, |entry_idx| self.match_kv_key(&bucket.entry(entry_idx), key));

            match probe_res {
                ProbeResult::Found(target_entry_idx) => {
                    let old_entry = bucket.entry(target_entry_idx);
                    if old_entry.is_list() {
                        return Err(TurboError::InvalidType("cannot overwrite list key with KV"));
                    }

                    let (old_pool_hop, old_local_idx) =
                        DBuf::decompose_buf_idx(old_entry.buf_idx());
                    let old_dbuf_slot = self.locate_databuf_slot(old_pool_hop)? as usize;
                    let orig_old_dbuf =
                        self.mmap.read_exact::<DBuf, _, _>(old_dbuf_slot, |d| *d)?;
                    let old_klen = orig_old_dbuf.read_header_klen(old_local_idx).unwrap_or(0);
                    let old_vlen = orig_old_dbuf
                        .read_header_vlen_or_adjarr_index(old_local_idx)
                        .unwrap_or(0) as usize;
                    let old_req = DBuf::required_kv_buffers(old_klen, old_vlen);

                    let target_global_buf: u32;
                    let mut touched_dbufs: Vec<(usize, DBuf, DBuf)>;

                    if req <= DBUF_BUF_COUNT
                        && old_req <= DBUF_BUF_COUNT
                        && (old_local_idx + old_req <= DBUF_BUF_COUNT)
                    {
                        touched_dbufs = Vec::with_capacity(2);

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
                        } else if let Some((mut freed_old, new_local)) = {
                            let mut f = orig_old_dbuf;
                            f.free_contiguous(old_local_idx, old_req);
                            f.alloc_contiguous(req).map(|l| (f, l))
                        } {
                            freed_old
                                .write_kv_payload(new_local, key, val)
                                .map_err(|_| TurboError::PayloadTooLarge)?;

                            target_global_buf = DBuf::global_buf_idx(old_pool_hop, new_local);
                            touched_dbufs.push((old_dbuf_slot, orig_old_dbuf, freed_old));
                        } else {
                            let mut freed_old = orig_old_dbuf;
                            freed_old.free_contiguous(old_local_idx, old_req);

                            let mut plan = self.find_free_databuf_span(req, &[old_dbuf_slot])?;
                            let mut mutated_refs: Vec<DBuf> =
                                plan.pools.iter().map(|(_, _, m)| *m).collect();

                            write_kv_payload_spanned(
                                &mut mutated_refs,
                                plan.start_global_buf,
                                key,
                                val,
                            )
                            .map_err(|_| TurboError::PayloadTooLarge)?;

                            for (i, (_, _, m)) in plan.pools.iter_mut().enumerate() {
                                *m = mutated_refs[i];
                            }

                            target_global_buf = plan.start_global_buf;

                            touched_dbufs.push((old_dbuf_slot, orig_old_dbuf, freed_old));
                            for p in plan.pools {
                                if let Some(existing) =
                                    touched_dbufs.iter_mut().find(|(s, _, _)| *s == p.0)
                                {
                                    existing.2 = p.2;
                                } else {
                                    touched_dbufs.push(p);
                                }
                            }
                        }
                    } else {
                        let old_start_pool = old_pool_hop;
                        let old_end_pool =
                            (old_entry.buf_idx() + (old_req as u32) - 1) / (DBUF_BUF_COUNT as u32);

                        if req <= old_req {
                            let mut pools =
                                Vec::with_capacity((old_end_pool - old_start_pool + 1) as usize);

                            for p in old_start_pool..=old_end_pool {
                                let slot = self.locate_databuf_slot(p)? as usize;
                                let orig = self.mmap.read_exact::<DBuf, _, _>(slot, |d| *d)?;
                                pools.push((slot, orig, orig));
                            }

                            if req < old_req {
                                let mut remaining_unmark = old_req - req;
                                let unmark_start_global = old_entry.buf_idx() + (req as u32);
                                let unmark_start_pool =
                                    unmark_start_global / (DBUF_BUF_COUNT as u32);
                                let unmark_local =
                                    (unmark_start_global % (DBUF_BUF_COUNT as u32)) as usize;

                                for p in unmark_start_pool..=old_end_pool {
                                    let pool_idx = (p - old_start_pool) as usize;
                                    let p_local =
                                        if p == unmark_start_pool { unmark_local } else { 0 };
                                    let count =
                                        std::cmp::min(DBUF_BUF_COUNT - p_local, remaining_unmark);

                                    unmark_spanned_buffers(&mut pools[pool_idx].2, p_local, count);
                                    remaining_unmark -= count;
                                    if remaining_unmark == 0 {
                                        break;
                                    }
                                }
                            }

                            let mut mutated_refs: Vec<DBuf> =
                                pools.iter().map(|(_, _, m)| *m).collect();

                            write_kv_payload_spanned(
                                &mut mutated_refs,
                                old_entry.buf_idx(),
                                key,
                                val,
                            )
                            .map_err(|_| TurboError::PayloadTooLarge)?;

                            for (i, (_, _, m)) in pools.iter_mut().enumerate() {
                                *m = mutated_refs[i];
                            }

                            target_global_buf = old_entry.buf_idx();
                            touched_dbufs = pools;
                        } else {
                            let mut old_pools =
                                Vec::with_capacity((old_end_pool - old_start_pool + 1) as usize);
                            let mut remaining_free = old_req;

                            for p in old_start_pool..=old_end_pool {
                                let slot = self.locate_databuf_slot(p)? as usize;
                                let orig = self.mmap.read_exact::<DBuf, _, _>(slot, |d| *d)?;

                                let mut mutated = orig;
                                let p_local = if p == old_start_pool { old_local_idx } else { 0 };
                                let count = std::cmp::min(DBUF_BUF_COUNT - p_local, remaining_free);

                                unmark_spanned_buffers(&mut mutated, p_local, count);
                                remaining_free -= count;
                                old_pools.push((slot, orig, mutated));
                            }

                            let mut plan = self.find_free_databuf_span(req, &[])?;
                            let mut mutated_refs: Vec<DBuf> =
                                plan.pools.iter().map(|(_, _, m)| *m).collect();

                            write_kv_payload_spanned(
                                &mut mutated_refs,
                                plan.start_global_buf,
                                key,
                                val,
                            )
                            .map_err(|_| TurboError::PayloadTooLarge)?;

                            for (i, (_, _, m)) in plan.pools.iter_mut().enumerate() {
                                *m = mutated_refs[i];
                            }

                            target_global_buf = plan.start_global_buf;
                            touched_dbufs = old_pools;

                            for p in plan.pools {
                                if let Some(existing) =
                                    touched_dbufs.iter_mut().find(|(s, _, _)| *s == p.0)
                                {
                                    existing.2 = p.2;
                                } else {
                                    touched_dbufs.push(p);
                                }
                            }
                        }
                    }

                    let mut new_bucket = bucket;
                    new_bucket.set_kv(target_entry_idx, hash, target_global_buf);

                    let mut changes: Vec<(usize, Slot, Slot)> =
                        Vec::with_capacity(BUCKET_SLOTS + touched_dbufs.len() * DBUF_SLOTS);

                    for i in 0..BUCKET_SLOTS {
                        let orig_s = bucket.as_slots()[i];
                        let new_s = new_bucket.as_slots()[i];

                        if orig_s != new_s {
                            changes.push(((bucket_slot as usize) + i, orig_s, new_s));
                        }
                    }

                    for (dbuf_slot, orig_d, mut_d) in &touched_dbufs {
                        for i in 0..DBUF_SLOTS {
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
                        Ok(ticket) => return Ok(TurboTicket::new(ticket)),
                        Err(_) => {
                            Self::occ_backoff(attempt);
                            continue;
                        }
                    }
                }
                ProbeResult::Vacant(target_entry_idx) => {
                    let mut plan = self.find_free_databuf_span(req, &[])?;
                    let mut mutated_refs: Vec<DBuf> =
                        plan.pools.iter().map(|(_, _, m)| *m).collect();

                    write_kv_payload_spanned(&mut mutated_refs, plan.start_global_buf, key, val)
                        .map_err(|_| TurboError::PayloadTooLarge)?;

                    for (i, (_, _, m)) in plan.pools.iter_mut().enumerate() {
                        *m = mutated_refs[i];
                    }

                    let target_global_buf = plan.start_global_buf;

                    let mut new_bucket = bucket;
                    new_bucket.set_kv(target_entry_idx, hash, target_global_buf);

                    let mut changes: Vec<(usize, Slot, Slot)> =
                        Vec::with_capacity(BUCKET_SLOTS + plan.pools.len() * DBUF_SLOTS);

                    for i in 0..BUCKET_SLOTS {
                        let orig_s = bucket.as_slots()[i];
                        let new_s = new_bucket.as_slots()[i];

                        if orig_s != new_s {
                            changes.push(((bucket_slot as usize) + i, orig_s, new_s));
                        }
                    }

                    for (slot, orig, mutated) in &plan.pools {
                        for i in 0..DBUF_SLOTS {
                            let orig_s = orig.as_slots()[i];
                            let new_s = mutated.as_slots()[i];

                            if orig_s != new_s {
                                changes.push((slot + i, orig_s, new_s));
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
                            let _ = self.total_entries.fetch_add(1, Ordering::Relaxed);
                            return Ok(TurboTicket::new(ticket));
                        }
                        Err(_) => {
                            Self::occ_backoff(attempt);
                            continue;
                        }
                    }
                }
                ProbeResult::Full => {
                    if depth >= MAX_TRIE_DEPTH {
                        return Err(TurboError::CollisionLimit);
                    }

                    let mut plan = self.find_free_databuf_span(req, &[])?;
                    let mut mutated_refs: Vec<DBuf> =
                        plan.pools.iter().map(|(_, _, m)| *m).collect();

                    write_kv_payload_spanned(&mut mutated_refs, plan.start_global_buf, key, val)
                        .map_err(|_| TurboError::PayloadTooLarge)?;

                    for (i, (_, _, m)) in plan.pools.iter_mut().enumerate() {
                        *m = mutated_refs[i];
                    }

                    let target_global_buf = plan.start_global_buf;
                    let new_entry = BucketEntry::new(hash, target_global_buf);

                    let mut all_entries = Vec::with_capacity(BUCKET_ENTRIES + 1);
                    for i in 0..BUCKET_ENTRIES {
                        all_entries.push(bucket.entry(i));
                    }

                    all_entries.push(new_entry);

                    let orig_parent =
                        self.mmap.read_exact::<Node, _, _>(parent_slot as usize, |n| *n)?;

                    let (_new_node_slot, compacted_bucket, mutated_parent) = self
                        .split_and_rehash(
                            parent_slot,
                            parent_child_idx,
                            bucket_slot,
                            depth,
                            &all_entries,
                        )?;

                    let mut changes: Vec<(usize, Slot, Slot)> =
                        Vec::with_capacity(1 + BUCKET_SLOTS + plan.pools.len() * DBUF_SLOTS);

                    changes.push((
                        parent_slot as usize,
                        *orig_parent.as_slot(),
                        *mutated_parent.as_slot(),
                    ));

                    for i in 0..BUCKET_SLOTS {
                        let orig_s = bucket.as_slots()[i];
                        let new_s = compacted_bucket.as_slots()[i];

                        if orig_s != new_s {
                            changes.push(((bucket_slot as usize) + i, orig_s, new_s));
                        }
                    }

                    for (slot, orig, mutated) in &plan.pools {
                        for i in 0..DBUF_SLOTS {
                            let orig_s = orig.as_slots()[i];
                            let new_s = mutated.as_slots()[i];

                            if orig_s != new_s {
                                changes.push((slot + i, orig_s, new_s));
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
                            let _ = self.total_entries.fetch_add(1, Ordering::Relaxed);
                            return Ok(TurboTicket::new(ticket));
                        }
                        Err(_) => {
                            Self::occ_backoff(attempt);
                            continue;
                        }
                    }
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

        let hash = hash_kv_key(key);

        for attempt in 0..OCC_MAX_RETRIES {
            let (_, _, bucket_slot, _depth) = self.traverse_to_leaf(hash)?;
            let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;

            let matched_idx = bucket
                .probe_match(hash, |entry_idx| self.match_kv_key(&bucket.entry(entry_idx), key));

            match matched_idx {
                Some(target_entry_idx) => {
                    let entry = bucket.entry(target_entry_idx);
                    let (pool_hop, local_idx) = DBuf::decompose_buf_idx(entry.buf_idx());
                    let dbuf_slot = self.locate_databuf_slot(pool_hop)? as usize;
                    let orig_dbuf = self.mmap.read_exact::<DBuf, _, _>(dbuf_slot, |d| *d)?;
                    let klen = orig_dbuf.read_header_klen(local_idx).unwrap_or(0);
                    let vlen =
                        orig_dbuf.read_header_vlen_or_adjarr_index(local_idx).unwrap_or(0) as usize;
                    let req = DBuf::required_kv_buffers(klen, vlen);

                    let mut changes: Vec<(usize, Slot, Slot)> = Vec::new();

                    if req <= DBUF_BUF_COUNT && local_idx + req <= DBUF_BUF_COUNT {
                        let mut freed_dbuf = orig_dbuf;
                        freed_dbuf.free_contiguous(local_idx, req);

                        for i in 0..DBUF_SLOTS {
                            let orig_s = orig_dbuf.as_slots()[i];
                            let new_s = freed_dbuf.as_slots()[i];

                            if orig_s != new_s {
                                changes.push((dbuf_slot + i, orig_s, new_s));
                            }
                        }
                    } else {
                        let start_pool = pool_hop;
                        let end_pool =
                            (entry.buf_idx() + (req as u32) - 1) / (DBUF_BUF_COUNT as u32);

                        let mut remaining = req;
                        for p in start_pool..=end_pool {
                            let dbuf_slot = self.locate_databuf_slot(p)? as usize;
                            let orig_dbuf =
                                self.mmap.read_exact::<DBuf, _, _>(dbuf_slot, |d| *d)?;

                            let mut freed_dbuf = orig_dbuf;
                            let p_local = if p == start_pool { local_idx } else { 0 };
                            let count = std::cmp::min(DBUF_BUF_COUNT - p_local, remaining);

                            unmark_spanned_buffers(&mut freed_dbuf, p_local, count);
                            remaining -= count;

                            for i in 0..DBUF_SLOTS {
                                let orig_s = orig_dbuf.as_slots()[i];
                                let new_s = freed_dbuf.as_slots()[i];

                                if orig_s != new_s {
                                    changes.push((dbuf_slot + i, orig_s, new_s));
                                }
                            }
                        }
                    }

                    let mut new_bucket = bucket;
                    new_bucket.mark_tombstone(target_entry_idx);

                    for i in 0..BUCKET_SLOTS {
                        let orig_s = bucket.as_slots()[i];
                        let new_s = new_bucket.as_slots()[i];

                        if orig_s != new_s {
                            changes.push(((bucket_slot as usize) + i, orig_s, new_s));
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
                            let _ = self.total_entries.fetch_sub(1, Ordering::Relaxed);
                            return Ok(true);
                        }
                        Err(_) => {
                            Self::occ_backoff(attempt);
                            continue;
                        }
                    }
                }
                None => return Ok(false),
            }
        }

        Err(TurboError::ResourceBusy("OCC write conflict limit reached"))
    }

    /// Ref: `DESIGN.md` §7.4
    pub(crate) fn list_append(&self, key: &[u8], item: &[u8]) -> TurboResult<TurboTicket> {
        if key.is_empty() || key.len() > MAX_KEY_LEN || item.len() > MAX_VAL_LEN {
            return Err(TurboError::PayloadTooLarge);
        }

        let req_item = DBuf::required_list_val_buffers(item.len());

        let hash = hash_list_key(key);
        for attempt in 0..OCC_MAX_RETRIES {
            let (parent_slot, parent_child_idx, bucket_slot, depth) =
                self.traverse_to_leaf(hash)?;
            let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;

            let probe_res =
                bucket.probe(hash, |entry_idx| self.match_list_key(&bucket.entry(entry_idx), key));

            match probe_res {
                ProbeResult::Found(target_entry_idx) => {
                    let entry = bucket.entry(target_entry_idx);
                    if !entry.is_list() {
                        return Err(TurboError::InvalidType("Key is a KV entry"));
                    }

                    let mut item_plan = self.find_free_databuf_span(req_item, &[])?;
                    let mut mutated_refs: Vec<DBuf> =
                        item_plan.pools.iter().map(|(_, _, m)| *m).collect();

                    write_list_val_payload_spanned(
                        &mut mutated_refs,
                        item_plan.start_global_buf,
                        item,
                    )
                    .map_err(|_| TurboError::PayloadTooLarge)?;

                    for (i, (_, _, m)) in item_plan.pools.iter_mut().enumerate() {
                        *m = mutated_refs[i];
                    }

                    let item_buf_idx = item_plan.start_global_buf;

                    let mut curr_arr_idx = self.read_list_adjarr_idx(entry.buf_idx())?;
                    let mut prev_info: Option<(u32, usize, AdjArrayBlock, usize)> = None;
                    let mut is_unoccupied = false;

                    let (prev_opt, last_block_slot, last_orig_block, last_local_array) = loop {
                        let (block_hop, local_array) = decompose_relative_array_idx(curr_arr_idx);
                        let block_slot = self.locate_adjarr_block_slot(block_hop)? as usize;

                        let block =
                            self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot, |b| *b)?;

                        if !block.is_array_occupied(local_array) {
                            is_unoccupied = true;
                            break (None, 0, block, 0);
                        }

                        let next = block.array(local_array).next_array();
                        if next == 0 {
                            break (prev_info, block_slot, block, local_array);
                        }

                        prev_info = Some((curr_arr_idx, block_slot, block, local_array));
                        curr_arr_idx = next;
                    };

                    if is_unoccupied {
                        Self::occ_backoff(attempt);
                        continue;
                    }

                    let mut touched_blocks: Vec<(usize, AdjArrayBlock, AdjArrayBlock)> = Vec::new();
                    if !last_orig_block.array(last_local_array).is_full() {
                        let mut mutated_block = last_orig_block;
                        let _ = mutated_block.array_mut(last_local_array).push(item_buf_idx);

                        touched_blocks.push((last_block_slot, last_orig_block, mutated_block));
                    } else {
                        let mut mutated_last_block = last_orig_block;
                        if let Some(free_local_arr) = mutated_last_block.alloc_array() {
                            let (last_hop, _) = decompose_relative_array_idx(curr_arr_idx);
                            let new_arr_idx = relative_array_idx(last_hop, free_local_arr);

                            mutated_last_block
                                .array_mut(last_local_array)
                                .set_next_array(new_arr_idx);
                            let _ = mutated_last_block.array_mut(free_local_arr).push(item_buf_idx);

                            touched_blocks.push((
                                last_block_slot,
                                last_orig_block,
                                mutated_last_block,
                            ));
                        } else {
                            let (new_arr_idx, new_block_slot, new_orig_block, mut new_mut_block) =
                                self.find_free_adjarr_array(last_block_slot)?;

                            let (_, new_local_arr) = decompose_relative_array_idx(new_arr_idx);
                            let _ = new_mut_block.array_mut(new_local_arr).push(item_buf_idx);

                            mutated_last_block
                                .array_mut(last_local_array)
                                .set_next_array(new_arr_idx);

                            touched_blocks.push((
                                last_block_slot,
                                last_orig_block,
                                mutated_last_block,
                            ));

                            touched_blocks.push((new_block_slot, new_orig_block, new_mut_block));
                        }
                    }

                    if let Some((_, prev_block_slot, prev_orig_block, _)) = prev_opt
                        && prev_block_slot != last_block_slot
                        && !touched_blocks.iter().any(|(s, _, _)| *s == prev_block_slot)
                    {
                        touched_blocks.push((prev_block_slot, prev_orig_block, prev_orig_block));
                    }

                    let mut changes: Vec<(usize, Slot, Slot)> = Vec::new();
                    for i in 0..BUCKET_SLOTS {
                        let s = bucket.as_slots()[i];
                        changes.push(((bucket_slot as usize) + i, s, s));
                    }

                    for (bs, orig_b, mut_b) in &touched_blocks {
                        for i in 0..ADJ_ARR_SLOTS {
                            let orig_s = orig_b.as_slots()[i];
                            let new_s = mut_b.as_slots()[i];

                            changes.push((*bs + i, orig_s, new_s));
                        }
                    }

                    for (slot, orig, mutated) in &item_plan.pools {
                        for i in 0..DBUF_SLOTS {
                            let orig_s = orig.as_slots()[i];
                            let new_s = mutated.as_slots()[i];

                            if orig_s != new_s {
                                changes.push((slot + i, orig_s, new_s));
                            }
                        }
                    }

                    changes.sort_by_key(|(s, _, _)| *s);
                    changes.dedup_by(|a, b| a.0 == b.0);
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
                }
                ProbeResult::Vacant(target_entry_idx) => {
                    let req_key = DBuf::required_list_key_buffers(key.len());

                    let mut key_plan = self.find_free_databuf_span(req_key, &[])?;
                    let mut key_mutated_refs: Vec<DBuf> =
                        key_plan.pools.iter().map(|(_, _, m)| *m).collect();

                    let key_buf_idx = key_plan.start_global_buf;
                    let key_slots: Vec<usize> = key_plan.pools.iter().map(|(s, _, _)| *s).collect();

                    let mut item_plan = self.find_free_databuf_span(req_item, &key_slots)?;
                    let mut item_mutated_refs: Vec<DBuf> =
                        item_plan.pools.iter().map(|(_, _, m)| *m).collect();

                    write_list_val_payload_spanned(
                        &mut item_mutated_refs,
                        item_plan.start_global_buf,
                        item,
                    )
                    .map_err(|_| TurboError::PayloadTooLarge)?;

                    for (i, (_, _, m)) in item_plan.pools.iter_mut().enumerate() {
                        *m = item_mutated_refs[i];
                    }

                    let item_buf_idx = item_plan.start_global_buf;

                    let (arr_idx, block_slot, orig_block, mut mut_block) =
                        self.find_free_adjarr_array(0)?;

                    let (_, local_arr) = decompose_relative_array_idx(arr_idx);
                    let _ = mut_block.array_mut(local_arr).push(item_buf_idx);

                    write_list_key_payload_spanned(
                        &mut key_mutated_refs,
                        key_plan.start_global_buf,
                        key,
                        arr_idx,
                    )
                    .map_err(|_| TurboError::PayloadTooLarge)?;

                    for (i, (_, _, m)) in key_plan.pools.iter_mut().enumerate() {
                        *m = key_mutated_refs[i];
                    }

                    let mut new_bucket = bucket;
                    new_bucket.set_list(target_entry_idx, hash, key_buf_idx);

                    let mut changes: Vec<(usize, Slot, Slot)> = Vec::new();
                    for i in 0..BUCKET_SLOTS {
                        let orig_s = bucket.as_slots()[i];
                        let new_s = new_bucket.as_slots()[i];

                        if orig_s != new_s {
                            changes.push(((bucket_slot as usize) + i, orig_s, new_s));
                        }
                    }

                    for i in 0..ADJ_ARR_SLOTS {
                        let orig_s = orig_block.as_slots()[i];
                        let new_s = mut_block.as_slots()[i];

                        if orig_s != new_s {
                            changes.push((block_slot + i, orig_s, new_s));
                        }
                    }

                    for (slot, orig, mutated) in &key_plan.pools {
                        for i in 0..DBUF_SLOTS {
                            let orig_s = orig.as_slots()[i];
                            let new_s = mutated.as_slots()[i];

                            if orig_s != new_s {
                                changes.push((slot + i, orig_s, new_s));
                            }
                        }
                    }

                    for (slot, orig, mutated) in &item_plan.pools {
                        for i in 0..DBUF_SLOTS {
                            let orig_s = orig.as_slots()[i];
                            let new_s = mutated.as_slots()[i];

                            if orig_s != new_s {
                                changes.push((slot + i, orig_s, new_s));
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
                            let _ = self.total_entries.fetch_add(1, Ordering::Relaxed);
                            return Ok(TurboTicket::new(ticket));
                        }
                        Err(_) => {
                            Self::occ_backoff(attempt);
                            continue;
                        }
                    }
                }
                ProbeResult::Full => {
                    if depth >= MAX_TRIE_DEPTH {
                        return Err(TurboError::CollisionLimit);
                    }

                    let req_key = DBuf::required_list_key_buffers(key.len());

                    let mut key_plan = self.find_free_databuf_span(req_key, &[])?;
                    let mut key_mutated_refs: Vec<DBuf> =
                        key_plan.pools.iter().map(|(_, _, m)| *m).collect();

                    let key_buf_idx = key_plan.start_global_buf;
                    let key_slots: Vec<usize> = key_plan.pools.iter().map(|(s, _, _)| *s).collect();

                    let mut item_plan = self.find_free_databuf_span(req_item, &key_slots)?;
                    let mut item_mutated_refs: Vec<DBuf> =
                        item_plan.pools.iter().map(|(_, _, m)| *m).collect();

                    write_list_val_payload_spanned(
                        &mut item_mutated_refs,
                        item_plan.start_global_buf,
                        item,
                    )
                    .map_err(|_| TurboError::PayloadTooLarge)?;

                    for (i, (_, _, m)) in item_plan.pools.iter_mut().enumerate() {
                        *m = item_mutated_refs[i];
                    }

                    let item_buf_idx = item_plan.start_global_buf;

                    let (arr_idx, block_slot, orig_block, mut mut_block) =
                        self.find_free_adjarr_array(0)?;

                    let (_, local_arr) = decompose_relative_array_idx(arr_idx);
                    let _ = mut_block.array_mut(local_arr).push(item_buf_idx);

                    write_list_key_payload_spanned(
                        &mut key_mutated_refs,
                        key_plan.start_global_buf,
                        key,
                        arr_idx,
                    )
                    .map_err(|_| TurboError::PayloadTooLarge)?;

                    for (i, (_, _, m)) in key_plan.pools.iter_mut().enumerate() {
                        *m = key_mutated_refs[i];
                    }

                    let new_entry = BucketEntry::new(hash, key_buf_idx);

                    let mut all_entries = Vec::with_capacity(BUCKET_ENTRIES + 1);
                    for i in 0..BUCKET_ENTRIES {
                        all_entries.push(bucket.entry(i));
                    }

                    all_entries.push(new_entry);

                    let orig_parent =
                        self.mmap.read_exact::<Node, _, _>(parent_slot as usize, |n| *n)?;
                    let (_new_node_slot, compacted_bucket, mutated_parent) = self
                        .split_and_rehash(
                            parent_slot,
                            parent_child_idx,
                            bucket_slot,
                            depth,
                            &all_entries,
                        )?;

                    let mut changes: Vec<(usize, Slot, Slot)> = Vec::new();
                    changes.push((
                        parent_slot as usize,
                        *orig_parent.as_slot(),
                        *mutated_parent.as_slot(),
                    ));

                    for i in 0..BUCKET_SLOTS {
                        let orig_s = bucket.as_slots()[i];
                        let new_s = compacted_bucket.as_slots()[i];

                        if orig_s != new_s {
                            changes.push(((bucket_slot as usize) + i, orig_s, new_s));
                        }
                    }

                    for i in 0..ADJ_ARR_SLOTS {
                        let orig_s = orig_block.as_slots()[i];
                        let new_s = mut_block.as_slots()[i];

                        if orig_s != new_s {
                            changes.push((block_slot + i, orig_s, new_s));
                        }
                    }

                    for (slot, orig, mutated) in &key_plan.pools {
                        for i in 0..DBUF_SLOTS {
                            let orig_s = orig.as_slots()[i];
                            let new_s = mutated.as_slots()[i];

                            if orig_s != new_s {
                                changes.push((slot + i, orig_s, new_s));
                            }
                        }
                    }

                    for (slot, orig, mutated) in &item_plan.pools {
                        for i in 0..DBUF_SLOTS {
                            let orig_s = orig.as_slots()[i];
                            let new_s = mutated.as_slots()[i];

                            if orig_s != new_s {
                                changes.push((slot + i, orig_s, new_s));
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
                            let _ = self.total_entries.fetch_add(1, Ordering::Relaxed);
                            return Ok(TurboTicket::new(ticket));
                        }
                        Err(_) => {
                            Self::occ_backoff(attempt);
                            continue;
                        }
                    }
                }
            }
        }

        Err(TurboError::ResourceBusy("OCC write conflict limit reached"))
    }

    fn find_free_adjarr_array(
        &self,
        exclude_slot: usize,
    ) -> TurboResult<(u32, usize, AdjArrayBlock, AdjArrayBlock)> {
        let mut curr = ADJ_ARR_HEAD_SLOT as usize;
        let mut last_slot = curr;
        let mut block_hop = 0u32;

        while curr != 0 {
            last_slot = curr;
            if curr != exclude_slot {
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
            } else {
                let next = self
                    .mmap
                    .read_exact::<AdjArrayBlock, _, _>(curr, |b| b.next_block() as usize)?;

                curr = next;
            }

            block_hop += 1;
        }

        let _guard = self.growth_mutex.lock().unwrap_or_else(|e| e.into_inner());

        let mut actual_tail = last_slot;
        let mut actual_hop = block_hop.saturating_sub(1);

        loop {
            let next = self
                .mmap
                .read_exact::<AdjArrayBlock, _, _>(actual_tail, |b| b.next_block() as usize)?;

            if next == 0 {
                break;
            }

            actual_tail = next;
            actual_hop += 1;

            if actual_tail != exclude_slot {
                let (_, opt_alloc) =
                    self.mmap.read_exact::<AdjArrayBlock, _, _>(actual_tail, |b| {
                        let next = b.next_block() as usize;
                        let mut mutated = *b;

                        match mutated.alloc_array() {
                            Some(local_idx) => (next, Some((local_idx, *b, mutated))),
                            None => (next, None),
                        }
                    })?;

                if let Some((local_idx, orig, mutated)) = opt_alloc {
                    let relative_idx = relative_array_idx(actual_hop, local_idx);
                    return Ok((relative_idx, actual_tail, orig, mutated));
                }
            }
        }

        let new_slot = self.expand_adjarr_locked(actual_tail)? as usize;

        let mut block = self.mmap.read_exact::<AdjArrayBlock, _, _>(new_slot, |b| *b)?;
        let orig = block;

        let local_idx = block.alloc_array().ok_or(TurboError::DatabaseFull)?;
        let relative_idx = relative_array_idx(actual_hop + 1, local_idx);

        Ok((relative_idx, new_slot, orig, block))
    }

    fn match_list_key(&self, entry: &BucketEntry, key: &[u8]) -> bool {
        if !entry.is_list() {
            return false;
        }

        let (pool_hop, local_idx) = DBuf::decompose_buf_idx(entry.buf_idx());
        let req_key = DBuf::required_list_key_buffers(key.len());

        if local_idx + req_key <= DBUF_BUF_COUNT {
            let Ok(dbuf_slot) = self.locate_databuf_slot(pool_hop) else {
                return false;
            };

            let Ok(matches) = self
                .mmap
                .read_exact::<DBuf, _, _>(dbuf_slot as usize, |d| d.verify_key(local_idx, key))
            else {
                return false;
            };

            matches
        } else {
            let start_pool = pool_hop;
            let end_pool = (entry.buf_idx() + (req_key as u32) - 1) / (DBUF_BUF_COUNT as u32);

            let mut dbufs = Vec::with_capacity((end_pool - start_pool + 1) as usize);
            for p in start_pool..=end_pool {
                let Ok(slot) = self.locate_databuf_slot(p) else {
                    return false;
                };

                let Ok(dbuf) = self.mmap.read_exact::<DBuf, _, _>(slot as usize, |d| *d) else {
                    return false;
                };

                dbufs.push(dbuf);
            }

            verify_list_key_spanned(&dbufs, entry.buf_idx(), key)
        }
    }

    /// Ref: `DESIGN.md` §7.4
    pub(crate) fn list_pop(&self, key: &[u8]) -> TurboResult<Option<Vec<u8>>> {
        if key.is_empty() || key.len() > MAX_KEY_LEN {
            return Ok(None);
        }

        let hash = hash_list_key(key);

        for attempt in 0..OCC_MAX_RETRIES {
            let (_, _, bucket_slot, _depth) = self.traverse_to_leaf(hash)?;
            let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;

            let matched_idx = bucket
                .probe_match(hash, |entry_idx| self.match_list_key(&bucket.entry(entry_idx), key));

            let target_entry_idx = match matched_idx {
                Some(idx) => idx,
                None => return Ok(None),
            };

            let entry = bucket.entry(target_entry_idx);

            let mut is_unoccupied = false;
            let mut curr_arr_idx = self.read_list_adjarr_idx(entry.buf_idx())?;
            let mut prev_info: Option<(u32, usize, AdjArrayBlock, usize)> = None;

            let (prev_opt, last_block_slot, last_orig_block, last_local_arr) = loop {
                let (block_hop, local_arr) = decompose_relative_array_idx(curr_arr_idx);
                let block_slot = self.locate_adjarr_block_slot(block_hop)? as usize;
                let block = self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot, |b| *b)?;

                if !block.is_array_occupied(local_arr) {
                    is_unoccupied = true;
                    break (None, 0, block, 0);
                }

                let next = block.array(local_arr).next_array();
                if next == 0 {
                    break (prev_info, block_slot, block, local_arr);
                }

                prev_info = Some((curr_arr_idx, block_slot, block, local_arr));
                curr_arr_idx = next;
            };

            if is_unoccupied {
                Self::occ_backoff(attempt);
                continue;
            }

            let mut mutated_last_block = last_orig_block;
            let popped_entry = match mutated_last_block.array_mut(last_local_arr).pop() {
                Some(e) => e,
                None => return Ok(None),
            };

            let (item_pool_hop, item_local_idx) = DBuf::decompose_buf_idx(popped_entry.buf_idx());

            let item_dbuf_slot = self.locate_databuf_slot(item_pool_hop)? as usize;
            let orig_item_dbuf = self.mmap.read_exact::<DBuf, _, _>(item_dbuf_slot, |d| *d)?;

            let vlen = orig_item_dbuf
                .read_list_vlen(item_local_idx)
                .map_err(|_| TurboError::CorruptedPayload("List item invalid index"))?;

            let req_item = DBuf::required_list_val_buffers(vlen);

            let val_bytes = if item_local_idx + req_item <= DBUF_BUF_COUNT {
                orig_item_dbuf
                    .read_list_val_payload(item_local_idx, vlen)
                    .map_err(|_| TurboError::CorruptedPayload("List item CRC mismatch"))?
                    .to_vec()
            } else {
                let start_pool = item_pool_hop;
                let end_pool =
                    (popped_entry.buf_idx() + (req_item as u32) - 1) / (DBUF_BUF_COUNT as u32);
                let mut dbufs = Vec::with_capacity((end_pool - start_pool + 1) as usize);
                dbufs.push(orig_item_dbuf);
                for p in (start_pool + 1)..=end_pool {
                    let slot = self.locate_databuf_slot(p)? as usize;
                    let next_dbuf = self.mmap.read_exact::<DBuf, _, _>(slot, |d| *d)?;
                    dbufs.push(next_dbuf);
                }
                read_list_val_payload_spanned(&dbufs, popped_entry.buf_idx(), vlen)
                    .map_err(|_| TurboError::CorruptedPayload("List item CRC mismatch"))?
            };

            let mut freed_dbufs: Vec<(usize, DBuf, DBuf)> = Vec::new();
            if item_local_idx + req_item <= DBUF_BUF_COUNT {
                let mut freed_item_dbuf = orig_item_dbuf;
                freed_item_dbuf.free_contiguous(item_local_idx, req_item);
                freed_dbufs.push((item_dbuf_slot, orig_item_dbuf, freed_item_dbuf));
            } else {
                let start_pool = item_pool_hop;
                let end_pool =
                    (popped_entry.buf_idx() + (req_item as u32) - 1) / (DBUF_BUF_COUNT as u32);
                let mut remaining = req_item;
                for p in start_pool..=end_pool {
                    let slot = self.locate_databuf_slot(p)? as usize;
                    let orig_dbuf = if p == start_pool {
                        orig_item_dbuf
                    } else {
                        self.mmap.read_exact::<DBuf, _, _>(slot, |d| *d)?
                    };
                    let mut freed_dbuf = orig_dbuf;
                    let p_local = if p == start_pool { item_local_idx } else { 0 };
                    let count = std::cmp::min(DBUF_BUF_COUNT - p_local, remaining);
                    unmark_spanned_buffers(&mut freed_dbuf, p_local, count);
                    remaining -= count;
                    freed_dbufs.push((slot, orig_dbuf, freed_dbuf));
                }
            }

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
                if let Some((_, prev_block_slot, prev_orig_block, _)) = prev_opt
                    && prev_block_slot != last_block_slot
                    && !touched_blocks.iter().any(|(s, _, _)| *s == prev_block_slot)
                {
                    touched_blocks.push((prev_block_slot, prev_orig_block, prev_orig_block));
                }
            }

            let mut changes: Vec<(usize, Slot, Slot)> = Vec::new();
            for i in 0..BUCKET_SLOTS {
                let s = bucket.as_slots()[i];
                changes.push(((bucket_slot as usize) + i, s, s));
            }

            for (bs, orig_b, mut_b) in &touched_blocks {
                for i in 0..ADJ_ARR_SLOTS {
                    let orig_s = orig_b.as_slots()[i];
                    let new_s = mut_b.as_slots()[i];

                    changes.push((*bs + i, orig_s, new_s));
                }
            }

            for (dbuf_slot, orig_d, freed_d) in &freed_dbufs {
                for i in 0..DBUF_SLOTS {
                    let orig_s = orig_d.as_slots()[i];
                    let new_s = freed_d.as_slots()[i];

                    if orig_s != new_s {
                        changes.push((dbuf_slot + i, orig_s, new_s));
                    }
                }
            }

            changes.sort_by_key(|(s, _, _)| *s);
            changes.dedup_by(|a, b| a.0 == b.0);

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

        let hash = hash_list_key(key);
        let (_, _, bucket_slot, _depth) = self.traverse_to_leaf(hash)?;

        let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;
        let matched_idx = bucket
            .probe_match(hash, |entry_idx| self.match_list_key(&bucket.entry(entry_idx), key));

        let target_entry_idx = match matched_idx {
            Some(idx) => idx,
            None => return Ok(None),
        };

        let entry = bucket.entry(target_entry_idx);
        let adjarr_idx = self.read_list_adjarr_idx(entry.buf_idx())?;
        let (block_hop, local_arr) = decompose_relative_array_idx(adjarr_idx);

        let block_slot = self.locate_adjarr_block_slot(block_hop)? as usize;
        let block = self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot, |b| *b)?;

        let Some(first_entry) = block.array(local_arr).entry(0) else {
            return Ok(None);
        };

        if first_entry.is_empty() {
            return Ok(None);
        }

        self.read_list_item_payload(first_entry.buf_idx()).map(Some)
    }

    /// Ref: `DESIGN.md` §7.4
    pub(crate) fn list_last(&self, key: &[u8]) -> TurboResult<Option<Vec<u8>>> {
        if key.is_empty() || key.len() > MAX_KEY_LEN {
            return Ok(None);
        }

        let hash = hash_list_key(key);
        let (_, _, bucket_slot, _depth) = self.traverse_to_leaf(hash)?;
        let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;

        let matched_idx = bucket
            .probe_match(hash, |entry_idx| self.match_list_key(&bucket.entry(entry_idx), key));

        let target_entry_idx = match matched_idx {
            Some(idx) => idx,
            None => return Ok(None),
        };

        let entry = bucket.entry(target_entry_idx);
        let mut curr_arr_idx = self.read_list_adjarr_idx(entry.buf_idx())?;
        let mut last_occupied_buf_idx: Option<u32> = None;

        loop {
            let (block_hop, local_arr) = decompose_relative_array_idx(curr_arr_idx);
            let block_slot = self.locate_adjarr_block_slot(block_hop)? as usize;
            let block = self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot, |b| *b)?;

            let array = block.array(local_arr);
            for i in (0..array.len()).rev() {
                if let Some(e) = array.entry(i)
                    && !e.is_empty()
                {
                    last_occupied_buf_idx = Some(e.buf_idx());
                    break;
                }
            }

            let next = array.next_array();
            if next == 0 {
                break;
            }

            curr_arr_idx = next;
        }

        match last_occupied_buf_idx {
            Some(buf_idx) => self.read_list_item_payload(buf_idx).map(Some),
            None => Ok(None),
        }
    }

    fn read_list_item_payload(&self, buf_idx: u32) -> TurboResult<Vec<u8>> {
        let (pool_hop, local_idx) = DBuf::decompose_buf_idx(buf_idx);
        let dbuf_slot = self.locate_databuf_slot(pool_hop)? as usize;
        let dbuf = self.mmap.read_exact::<DBuf, _, _>(dbuf_slot, |d| *d)?;

        let vlen = dbuf
            .read_list_vlen(local_idx)
            .map_err(|_| TurboError::CorruptedPayload("List item invalid index"))?;

        let req = DBuf::required_list_val_buffers(vlen);
        if local_idx + req <= DBUF_BUF_COUNT {
            let val = dbuf
                .read_list_val_payload(local_idx, vlen)
                .map_err(|_| TurboError::CorruptedPayload("List item CRC mismatch"))?;

            Ok(val.to_vec())
        } else {
            let start_pool = pool_hop;
            let end_pool = (buf_idx + (req as u32) - 1) / (DBUF_BUF_COUNT as u32);

            let mut dbufs = Vec::with_capacity((end_pool - start_pool + 1) as usize);
            dbufs.push(dbuf);

            for p in (start_pool + 1)..=end_pool {
                let slot = self.locate_databuf_slot(p)? as usize;
                let next_dbuf = self.mmap.read_exact::<DBuf, _, _>(slot, |d| *d)?;
                dbufs.push(next_dbuf);
            }

            read_list_val_payload_spanned(&dbufs, buf_idx, vlen)
                .map_err(|_| TurboError::CorruptedPayload("List item CRC mismatch"))
        }
    }

    /// Ref: `DESIGN.md` §7.4
    pub(crate) fn list_get_at(&self, key: &[u8], idx: usize) -> TurboResult<Option<Vec<u8>>> {
        if key.is_empty() || key.len() > MAX_KEY_LEN {
            return Ok(None);
        }

        let hash = hash_list_key(key);

        for attempt in 0..OCC_MAX_RETRIES {
            let (_, _, bucket_slot, _depth) = self.traverse_to_leaf(hash)?;
            let orig_bucket_slot = bucket_slot as usize;

            let orig_slot = self.mmap.read_exact::<Slot, _, _>(orig_bucket_slot, |s| *s)?;
            let bucket = self.mmap.read_exact::<Bucket, _, _>(orig_bucket_slot, |b| *b)?;

            let matched_idx = bucket
                .probe_match(hash, |entry_idx| self.match_list_key(&bucket.entry(entry_idx), key));

            let target_entry_idx = match matched_idx {
                Some(i) => i,
                None => return Ok(None),
            };

            let entry = bucket.entry(target_entry_idx);
            let mut curr_arr_idx = self.read_list_adjarr_idx(entry.buf_idx())?;
            let mut current_offset = 0usize;
            let mut result_payload: Option<Option<Vec<u8>>> = None;
            let mut retry_needed = false;

            loop {
                let (block_hop, local_arr) = decompose_relative_array_idx(curr_arr_idx);
                let block_slot = match self.locate_adjarr_block_slot(block_hop) {
                    Ok(s) => s as usize,
                    Err(_) => {
                        retry_needed = true;
                        break;
                    }
                };

                let block = match self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot, |b| *b) {
                    Ok(b) => b,
                    Err(_) => {
                        retry_needed = true;
                        break;
                    }
                };

                let array = block.array(local_arr);
                let array_len = array.len();

                if idx < current_offset + array_len {
                    let local_entry_idx = idx - current_offset;
                    if let Some(e) = array.entry(local_entry_idx) {
                        if !e.is_empty() {
                            match self.read_list_item_payload(e.buf_idx()) {
                                Ok(p) => result_payload = Some(Some(p)),
                                Err(_) => {
                                    retry_needed = true;
                                    break;
                                }
                            }
                        } else {
                            result_payload = Some(None);
                        }
                    } else {
                        result_payload = Some(None);
                    }
                    break;
                }

                current_offset += array_len;
                let next = array.next_array();
                if next == 0 {
                    break;
                }
                curr_arr_idx = next;
            }

            let current_slot = self.mmap.read_exact::<Slot, _, _>(orig_bucket_slot, |s| *s)?;
            if !retry_needed && current_slot == orig_slot {
                return Ok(result_payload.unwrap_or(None));
            }

            Self::occ_backoff(attempt);
        }

        let (_, _, bucket_slot, _depth) = self.traverse_to_leaf(hash)?;
        let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;

        if bucket.probe_match(hash, |i| self.match_list_key(&bucket.entry(i), key)).is_none() {
            Ok(None)
        } else {
            Err(TurboError::ResourceBusy("list get_at retry limit exceeded"))
        }
    }

    /// Ref: `DESIGN.md` §7.4
    pub(crate) fn list_len(&self, key: &[u8]) -> TurboResult<usize> {
        if key.is_empty() || key.len() > MAX_KEY_LEN {
            return Ok(0);
        }

        let hash = hash_list_key(key);

        for attempt in 0..OCC_MAX_RETRIES {
            let (_, _, bucket_slot, _depth) = self.traverse_to_leaf(hash)?;
            let orig_bucket_slot = bucket_slot as usize;
            let orig_slot = self.mmap.read_exact::<Slot, _, _>(orig_bucket_slot, |s| *s)?;
            let bucket = self.mmap.read_exact::<Bucket, _, _>(orig_bucket_slot, |b| *b)?;

            let matched_idx = bucket
                .probe_match(hash, |entry_idx| self.match_list_key(&bucket.entry(entry_idx), key));

            let target_entry_idx = match matched_idx {
                Some(idx) => idx,
                None => return Ok(0),
            };

            let entry = bucket.entry(target_entry_idx);
            let mut curr_arr_idx = self.read_list_adjarr_idx(entry.buf_idx())?;
            let mut total_count = 0usize;
            let mut retry_needed = false;

            loop {
                let (block_hop, local_arr) = decompose_relative_array_idx(curr_arr_idx);
                let block_slot = match self.locate_adjarr_block_slot(block_hop) {
                    Ok(s) => s as usize,
                    Err(_) => {
                        retry_needed = true;
                        break;
                    }
                };

                let block = match self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot, |b| *b) {
                    Ok(b) => b,
                    Err(_) => {
                        retry_needed = true;
                        break;
                    }
                };

                let array = block.array(local_arr);
                total_count += array.len();

                let next = array.next_array();
                if next == 0 {
                    break;
                }
                curr_arr_idx = next;
            }

            let current_slot = self.mmap.read_exact::<Slot, _, _>(orig_bucket_slot, |s| *s)?;
            if !retry_needed && current_slot == orig_slot {
                return Ok(total_count);
            }

            Self::occ_backoff(attempt);
        }

        let (_, _, bucket_slot, _depth) = self.traverse_to_leaf(hash)?;
        let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;
        if bucket.probe_match(hash, |idx| self.match_list_key(&bucket.entry(idx), key)).is_none() {
            Ok(0)
        } else {
            Err(TurboError::ResourceBusy("list len retry limit exceeded"))
        }
    }

    /// Ref: `DESIGN.md` §7.4
    pub(crate) fn list_items(&self, key: &[u8]) -> TurboResult<Vec<Vec<u8>>> {
        if key.is_empty() || key.len() > MAX_KEY_LEN {
            return Ok(Vec::new());
        }

        let hash = hash_list_key(key);

        for attempt in 0..OCC_MAX_RETRIES {
            let (_, _, bucket_slot, _depth) = self.traverse_to_leaf(hash)?;
            let orig_bucket_slot = bucket_slot as usize;
            let orig_slot = self.mmap.read_exact::<Slot, _, _>(orig_bucket_slot, |s| *s)?;
            let bucket = self.mmap.read_exact::<Bucket, _, _>(orig_bucket_slot, |b| *b)?;

            let matched_idx = bucket
                .probe_match(hash, |entry_idx| self.match_list_key(&bucket.entry(entry_idx), key));

            let target_entry_idx = match matched_idx {
                Some(idx) => idx,
                None => return Ok(Vec::new()),
            };

            let entry = bucket.entry(target_entry_idx);
            let mut curr_arr_idx = self.read_list_adjarr_idx(entry.buf_idx())?;
            let mut items = Vec::new();
            let mut retry_needed = false;

            loop {
                let (block_hop, local_arr) = decompose_relative_array_idx(curr_arr_idx);
                let block_slot = match self.locate_adjarr_block_slot(block_hop) {
                    Ok(s) => s as usize,
                    Err(_) => {
                        retry_needed = true;
                        break;
                    }
                };

                let block = match self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot, |b| *b) {
                    Ok(b) => b,
                    Err(_) => {
                        retry_needed = true;
                        break;
                    }
                };

                let array = block.array(local_arr);
                for i in 0..array.len() {
                    if let Some(e) = array.entry(i)
                        && !e.is_empty()
                    {
                        match self.read_list_item_payload(e.buf_idx()) {
                            Ok(payload) => items.push(payload),
                            Err(_) => {
                                retry_needed = true;
                                break;
                            }
                        }
                    }
                }

                if retry_needed {
                    break;
                }

                let next = array.next_array();
                if next == 0 {
                    break;
                }
                curr_arr_idx = next;
            }

            let current_slot = self.mmap.read_exact::<Slot, _, _>(orig_bucket_slot, |s| *s)?;
            if !retry_needed && current_slot == orig_slot {
                return Ok(items);
            }

            Self::occ_backoff(attempt);
        }

        let (_, _, bucket_slot, _depth) = self.traverse_to_leaf(hash)?;
        let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;

        if bucket.probe_match(hash, |idx| self.match_list_key(&bucket.entry(idx), key)).is_none() {
            Ok(Vec::new())
        } else {
            Err(TurboError::ResourceBusy("list items retry limit exceeded"))
        }
    }

    /// Ref: `DESIGN.md` §7.4
    pub(crate) fn list_del(&self, key: &[u8]) -> TurboResult<bool> {
        if key.is_empty() || key.len() > MAX_KEY_LEN {
            return Ok(false);
        }

        let hash = hash_list_key(key);

        for attempt in 0..OCC_MAX_RETRIES {
            let (_, _, bucket_slot, _depth) = self.traverse_to_leaf(hash)?;
            let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;

            let matched_idx = bucket
                .probe_match(hash, |entry_idx| self.match_list_key(&bucket.entry(entry_idx), key));

            match matched_idx {
                Some(target_entry_idx) => {
                    let entry = bucket.entry(target_entry_idx);

                    let (key_pool_hop, key_local_idx) = DBuf::decompose_buf_idx(entry.buf_idx());
                    let req_key = DBuf::required_list_key_buffers(key.len());

                    let mut touched_dbufs: std::collections::HashMap<usize, (DBuf, DBuf)> =
                        std::collections::HashMap::new();

                    let key_start_pool = key_pool_hop;
                    let key_end_pool =
                        (entry.buf_idx() + (req_key as u32) - 1) / (DBUF_BUF_COUNT as u32);
                    let mut remaining_key = req_key;

                    for p in key_start_pool..=key_end_pool {
                        let slot = self.locate_databuf_slot(p)? as usize;
                        let orig = self.mmap.read_exact::<DBuf, _, _>(slot, |d| *d)?;
                        let mut mutated = orig;
                        let p_local = if p == key_start_pool { key_local_idx } else { 0 };
                        let count = std::cmp::min(DBUF_BUF_COUNT - p_local, remaining_key);
                        unmark_spanned_buffers(&mut mutated, p_local, count);
                        remaining_key -= count;
                        let _ = touched_dbufs.insert(slot, (orig, mutated));
                    }

                    let mut curr_arr_idx = self.read_list_adjarr_idx(entry.buf_idx())?;
                    let mut touched_blocks: Vec<(usize, AdjArrayBlock, AdjArrayBlock)> = Vec::new();

                    loop {
                        let (block_hop, local_array) = decompose_relative_array_idx(curr_arr_idx);
                        let block_slot = self.locate_adjarr_block_slot(block_hop)? as usize;
                        let orig_block = if let Some((_, _, mut_b)) =
                            touched_blocks.iter().find(|(s, _, _)| *s == block_slot)
                        {
                            *mut_b
                        } else {
                            self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot, |b| *b)?
                        };

                        let mut mutated_block = orig_block;
                        let next_arr = mutated_block.array(local_array).next_array();

                        for item_e in mutated_block.array(local_array).entries {
                            if !item_e.is_empty() {
                                let (item_pool_hop, item_local_idx) =
                                    DBuf::decompose_buf_idx(item_e.buf_idx());
                                let item_start_slot =
                                    self.locate_databuf_slot(item_pool_hop)? as usize;

                                let first_dbuf = if let Some((_, mut_d)) =
                                    touched_dbufs.get(&item_start_slot)
                                {
                                    *mut_d
                                } else {
                                    self.mmap.read_exact::<DBuf, _, _>(item_start_slot, |d| *d)?
                                };

                                let vlen = first_dbuf.read_list_vlen(item_local_idx).unwrap_or(0);
                                let req_item = DBuf::required_list_val_buffers(vlen);

                                let item_start_pool = item_pool_hop;
                                let item_end_pool = (item_e.buf_idx() + (req_item as u32) - 1)
                                    / (DBUF_BUF_COUNT as u32);
                                let mut remaining_item = req_item;

                                for p in item_start_pool..=item_end_pool {
                                    let slot = self.locate_databuf_slot(p)? as usize;
                                    let p_local =
                                        if p == item_start_pool { item_local_idx } else { 0 };
                                    let count =
                                        std::cmp::min(DBUF_BUF_COUNT - p_local, remaining_item);

                                    let mut_d =
                                        if let Some((_, mut_d)) = touched_dbufs.get_mut(&slot) {
                                            mut_d
                                        } else {
                                            let orig =
                                                self.mmap.read_exact::<DBuf, _, _>(slot, |d| *d)?;
                                            let _ = touched_dbufs.insert(slot, (orig, orig));
                                            &mut touched_dbufs.get_mut(&slot).unwrap().1
                                        };

                                    unmark_spanned_buffers(mut_d, p_local, count);
                                    remaining_item -= count;
                                }
                            }
                        }

                        mutated_block.free_array(local_array);
                        if let Some(pos) =
                            touched_blocks.iter().position(|(s, _, _)| *s == block_slot)
                        {
                            touched_blocks[pos].2 = mutated_block;
                        } else {
                            let actual_orig =
                                self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot, |b| *b)?;
                            touched_blocks.push((block_slot, actual_orig, mutated_block));
                        }

                        if next_arr == 0 {
                            break;
                        }
                        curr_arr_idx = next_arr;
                    }

                    let mut new_bucket = bucket;
                    new_bucket.mark_tombstone(target_entry_idx);

                    let mut changes: Vec<(usize, Slot, Slot)> = Vec::new();
                    for i in 0..BUCKET_SLOTS {
                        let orig_s = bucket.as_slots()[i];
                        let new_s = new_bucket.as_slots()[i];
                        if orig_s != new_s {
                            changes.push(((bucket_slot as usize) + i, orig_s, new_s));
                        }
                    }

                    for (bs, orig_b, mut_b) in &touched_blocks {
                        for i in 0..ADJ_ARR_SLOTS {
                            let orig_s = orig_b.as_slots()[i];
                            let new_s = mut_b.as_slots()[i];
                            if orig_s != new_s {
                                changes.push((*bs + i, orig_s, new_s));
                            }
                        }
                    }

                    for (dbuf_slot, (orig_d, mut_d)) in &touched_dbufs {
                        for i in 0..DBUF_SLOTS {
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
                        Ok(_) => {
                            let _ = self.total_entries.fetch_sub(1, Ordering::Relaxed);
                            return Ok(true);
                        }
                        Err(_) => {
                            Self::occ_backoff(attempt);
                            continue;
                        }
                    }
                }
                None => return Ok(false),
            }
        }

        Err(TurboError::ResourceBusy("OCC write conflict limit reached"))
    }

    /// Ref: `DESIGN.md` §8.1
    pub(crate) fn total_entries(&self) -> u64 {
        self.total_entries.load(Ordering::Acquire)
    }

    /// Ref: `DESIGN.md` §8.1
    pub(crate) fn resident_bytes_in_memory(&self) -> TurboResult<usize> {
        let mmap_bytes = self.mmap.resident_bytes()?;
        let cache_bytes = self.dbuf_slot_cache.as_ref().map_or(0, |c| c.memory_bytes());
        Ok(mmap_bytes.saturating_add(cache_bytes))
    }

    /// Ref: `DESIGN.md` §8.1
    #[inline]
    pub(crate) fn resident_bytes_on_disk(&self) -> usize {
        self.mmap.length()
    }

    /// Ref: `DESIGN.md` §8.1
    pub(crate) fn sync(&self) -> TurboResult<()> {
        let entries = self.total_entries.load(Ordering::Acquire);
        let _ = self.mmap.write_exact::<Metadata, _, _>(0, |m| {
            m.total_entries = entries;
        })?;

        self.mmap.sync()?;
        Ok(())
    }

    /// Ref: `DESIGN.md` §8.1
    pub(crate) fn evict(&self, mode: EvictionMode) -> TurboResult<()> {
        self.sync()?;

        let total_bytes = self.resident_bytes_on_disk();
        if total_bytes == 0x00 {
            return Ok(());
        }

        let page_size = self.sys_page_size.as_usize();
        let header_pages = match self.sys_page_size {
            PageSize::K4 => 0x02,
            PageSize::K16 | PageSize::K64 => 0x01,
        };

        match mode {
            EvictionMode::PurgeAll => {
                self.mmap.evict()?;
            }
            EvictionMode::PreserveHeaders => {
                let skip_bytes = header_pages * page_size;
                if total_bytes > skip_bytes {
                    let evict_len = total_bytes - skip_bytes;
                    self.mmap.evict_range(skip_bytes, evict_len)?;
                }
            }
        }

        if let Some(ref cache) = self.dbuf_slot_cache {
            cache.clear();
        }

        Ok(())
    }

    /// Ref: `DESIGN.md` §8.1
    pub(crate) fn delete(self) -> TurboResult<()> {
        self.mmap.delete()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::{Seek, SeekFrom, Write};
    use tempfile::tempdir;

    #[test]
    fn ok_engine_create_and_reopen() {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path());

        let engine = Engine::create(cfg.clone()).unwrap();
        assert_eq!(engine.total_entries(), 0);
        assert!(engine.resident_bytes_on_disk() > 0);

        let _ = engine.set(b"reopen_k", b"reopen_v").unwrap();
        assert_eq!(engine.total_entries(), 1);
        engine.sync().unwrap();
        drop(engine);

        let reopened = Engine::open(cfg).unwrap();
        assert_eq!(reopened.total_entries(), 1);
        assert_eq!(reopened.get(b"reopen_k").unwrap(), Some(b"reopen_v".to_vec()));
        assert!(reopened.contains_key(b"reopen_k").unwrap());
        reopened.delete().unwrap();
    }

    #[test]
    fn err_engine_open_validation_matrix() {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path());

        // 1: Missing backing directory
        let missing_dir = tempdir().unwrap();
        let missing_cfg = TurboCfg::manual(missing_dir.path());
        assert!(Engine::open(missing_cfg).is_err());

        // 2: Magic mismatch
        let engine = Engine::create(cfg.clone()).unwrap();
        engine.sync().unwrap();
        drop(engine);

        let db_file =
            std::fs::read_dir(dir.path()).unwrap().filter_map(|e| e.ok()).next().unwrap().path();

        {
            let mut file = OpenOptions::new().write(true).open(&db_file).unwrap();
            let _ = file.seek(SeekFrom::Start(0)).unwrap();
            file.write_all(&[0x00, 0x00, 0x00, 0x00]).unwrap();
            drop(file);

            assert!(matches!(Engine::open(cfg.clone()), Err(TurboError::InvalidMagic)));
        }

        // 3: Version mismatch
        {
            let mut file = OpenOptions::new().write(true).open(&db_file).unwrap();
            let _ = file.seek(SeekFrom::Start(0)).unwrap();
            file.write_all(&MAGIC).unwrap();
            let _ = file.seek(SeekFrom::Start(4)).unwrap();
            file.write_all(&0xFFu32.to_le_bytes()).unwrap();
            drop(file);

            assert!(matches!(
                Engine::open(cfg.clone()),
                Err(TurboError::VersionMismatch { expected: LAYOUT_VERSION, found: 0xFF })
            ));
        }

        // 4: Invalid page size
        {
            let mut file = OpenOptions::new().write(true).open(&db_file).unwrap();
            let _ = file.seek(SeekFrom::Start(4)).unwrap();
            file.write_all(&LAYOUT_VERSION.to_le_bytes()).unwrap();
            let _ = file.seek(SeekFrom::Start(8)).unwrap();
            file.write_all(&0x01u32.to_le_bytes()).unwrap();
            drop(file);

            assert!(matches!(Engine::open(cfg.clone()), Err(TurboError::InvalidPageSize(1))));
        }

        // 5: Total slots divergence
        {
            let mut file = OpenOptions::new().write(true).open(&db_file).unwrap();
            let page_size = MemMap::<Slot>::page_size().unwrap().as_u32();
            let _ = file.seek(SeekFrom::Start(8)).unwrap();
            file.write_all(&page_size.to_le_bytes()).unwrap();
            let _ = file.seek(SeekFrom::Start(0x10)).unwrap();
            file.write_all(&0x1234u32.to_le_bytes()).unwrap();
            drop(file);

            assert!(matches!(Engine::open(cfg.clone()), Err(TurboError::CorruptedHeader(_))));
        }

        // 6: Truncated file length
        {
            let file = OpenOptions::new().write(true).open(&db_file).unwrap();
            let len = file.metadata().unwrap().len();
            file.set_len(len - (SLOT_SIZE as u64)).unwrap();
            drop(file);

            assert!(matches!(Engine::open(cfg), Err(TurboError::Storage { .. })));
        }
    }

    #[test]
    fn ok_engine_uniform_slot_allocation_and_growth() {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path());
        let engine = Engine::create(cfg).unwrap();

        let initial_free = engine.free_slot_marker.load(Ordering::Acquire);
        let alloc_s = {
            let _guard = engine.growth_mutex.lock().unwrap();
            engine.alloc_uniform_slots_locked(0x20).unwrap()
        };
        assert_eq!(alloc_s, initial_free);
        assert_eq!(engine.free_slot_marker.load(Ordering::Acquire), initial_free + 0x20);

        let initial_disk = engine.resident_bytes_on_disk();
        let initial_slots = engine.total_slots.load(Ordering::Acquire);
        let huge_alloc = {
            let _guard = engine.growth_mutex.lock().unwrap();
            engine.alloc_uniform_slots_locked(initial_slots + 0x40).unwrap()
        };
        assert!(huge_alloc >= initial_free + 0x20);
        assert!(engine.resident_bytes_on_disk() > initial_disk);
        assert!(engine.total_slots.load(Ordering::Acquire) > initial_slots);

        engine.delete().unwrap();
    }

    #[test]
    fn err_engine_slot_index_exhaustion_on_u32_max() {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path());
        let engine = Engine::create(cfg).unwrap();

        // Simulate free_slot_marker reaching u32 boundary
        engine.free_slot_marker.store(0xFFFF_FFFE, Ordering::Release);
        {
            let _guard = engine.growth_mutex.lock().unwrap();
            let res = engine.alloc_uniform_slots_locked(4);
            assert!(matches!(res, Err(TurboError::DatabaseFull)));
        }

        engine.free_slot_marker.store(0xFFFF_FFFF, Ordering::Release);
        {
            let _guard = engine.growth_mutex.lock().unwrap();
            let res = engine.alloc_uniform_slots_locked(1);
            assert!(matches!(res, Err(TurboError::DatabaseFull)));
        }

        engine.delete().unwrap();
    }

    #[test]
    fn ok_engine_databuf_allocation_and_chaining() {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path());
        let engine = Engine::create(cfg).unwrap();

        assert_eq!(engine.databuf_slot_to_hop(DATABUF_HEAD_SLOT).unwrap(), 0);
        assert_eq!(engine.locate_databuf_slot(0).unwrap(), DATABUF_HEAD_SLOT);

        let second_slot = DATABUF_HEAD_SLOT + (DBUF_SLOTS as u32);
        assert_eq!(engine.locate_databuf_slot(1).unwrap(), second_slot);
        assert_eq!(engine.databuf_slot_to_hop(second_slot).unwrap(), 1);

        let last_init_slot =
            DATABUF_HEAD_SLOT + ((INITIAL_DATABUF_POOLS - 1) as u32) * (DBUF_SLOTS as u32);
        let new_slot = {
            let _guard = engine.growth_mutex.lock().unwrap();
            let s = engine.expand_databuf_locked(last_init_slot as usize, 1).unwrap();
            let new_hop = engine.databuf_slot_to_hop(s).unwrap();
            engine.active_dbuf_hint.store(new_hop, Ordering::Relaxed);
            s
        };
        assert_eq!(engine.locate_databuf_slot(INITIAL_DATABUF_POOLS as u32).unwrap(), new_slot);
        assert_eq!(engine.databuf_slot_to_hop(new_slot).unwrap(), INITIAL_DATABUF_POOLS as u32);

        let plan = engine.find_free_databuf(0x04, &[]).unwrap();
        assert_eq!(plan.start_slot, new_slot as usize);
        assert_eq!(plan.local_idx, 0);

        engine.delete().unwrap();
    }

    #[test]
    fn err_engine_broken_chain_detection() {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path());
        let engine = Engine::create(cfg).unwrap();

        // 1: Sever databuf chain
        let _ = engine.mmap.write_exact::<DBuf, _, _>(DATABUF_HEAD_SLOT as usize, |d| {
            d.set_next_dbuf_index(0);
        });
        assert!(matches!(
            engine.locate_databuf_slot(1),
            Err(TurboError::CorruptedHeader("broken databuf chain"))
        ));

        // 2: Non-existent slot hop lookup
        assert!(matches!(
            engine.databuf_slot_to_hop(0x9999),
            Err(TurboError::CorruptedHeader("databuf slot not found in allocation chain"))
        ));

        // 3: Sever adjarr chain
        let _ = engine.mmap.write_exact::<AdjArrayBlock, _, _>(ADJ_ARR_HEAD_SLOT as usize, |b| {
            b.set_next_block(0);
        });
        assert!(matches!(
            engine.locate_adjarr_block_slot(1),
            Err(TurboError::CorruptedHeader("broken adjarr block chain"))
        ));

        engine.delete().unwrap();
    }

    #[test]
    fn ok_engine_adjarr_allocation_and_chaining() {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path());
        let engine = Engine::create(cfg).unwrap();

        assert_eq!(engine.locate_adjarr_block_slot(0).unwrap(), ADJ_ARR_HEAD_SLOT);

        let (rel_idx, block_slot, _orig, _mutated) = engine.find_free_adjarr_array(0).unwrap();
        assert_eq!(block_slot, ADJ_ARR_HEAD_SLOT as usize);

        let (hop, local) = decompose_relative_array_idx(rel_idx);
        assert_eq!(hop, 0);
        assert_eq!(local, 0);

        let new_block_slot = {
            let _guard = engine.growth_mutex.lock().unwrap();
            engine.expand_adjarr_locked(ADJ_ARR_HEAD_SLOT as usize).unwrap()
        };
        assert!(new_block_slot > 0);

        engine.delete().unwrap();
    }

    #[test]
    fn ok_dbuf_slot_cache_push_and_get() {
        let cache = DBufSlotCache::new();
        assert_eq!(cache.get(0), None);
        assert_eq!(cache.get(0x10), None);

        cache.push(0, 0x40);
        assert_eq!(cache.get(0), Some(0x40));
        assert_eq!(cache.get(1), None);

        // Push across chunk boundary (chunk 1 starts at 4096 = 0x1000)
        cache.push(0x1000, 0x5000);
        assert_eq!(cache.get(0x1000), Some(0x5000));
        assert_eq!(cache.get(0), Some(0x40));
        assert_eq!(cache.get(0x0FFF), None);
    }

    #[test]
    fn ok_dbuf_slot_cache_memory_bytes() {
        let cache = DBufSlotCache::new();
        let base_size = core::mem::size_of::<DBufSlotCache>();
        assert_eq!(cache.memory_bytes(), base_size);

        cache.push(0, 0x40);
        let chunk_size = core::mem::size_of::<[AtomicU32; DBufSlotCache::CHUNK_SIZE]>();
        assert_eq!(cache.memory_bytes(), base_size + chunk_size);

        cache.push(0x1000, 0x5000);
        assert_eq!(cache.memory_bytes(), base_size + (chunk_size * 2));
    }

    #[test]
    fn ok_dbuf_slot_cache_clear() {
        let cache = DBufSlotCache::new();
        let base_size = core::mem::size_of::<DBufSlotCache>();

        cache.push(0x00, 0x40);
        cache.push(0x1000, 0x5000);
        let chunk_size = core::mem::size_of::<[AtomicU32; DBufSlotCache::CHUNK_SIZE]>();
        assert_eq!(cache.memory_bytes(), base_size + (chunk_size * 2));

        cache.clear();
        assert_eq!(cache.memory_bytes(), base_size);
        assert_eq!(cache.get(0x00), None);
        assert_eq!(cache.get(0x1000), None);
    }

    #[test]
    fn ok_engine_kv_crud_lifecycle() {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path());
        let engine = Engine::create(cfg).unwrap();

        assert!(!engine.contains_key(b"key_alpha").unwrap());
        assert_eq!(engine.get(b"key_alpha").unwrap(), None);

        // 1: Insert new KV
        let ticket = engine.set(b"key_alpha", b"val_init").unwrap();
        assert!(ticket.epoch() > 0);
        assert_eq!(engine.total_entries(), 1);
        assert!(engine.contains_key(b"key_alpha").unwrap());
        assert_eq!(engine.get(b"key_alpha").unwrap(), Some(b"val_init".to_vec()));

        // 2: In-place overwrite (smaller or equal payload)
        let _ = engine.set(b"key_alpha", b"v2").unwrap();
        assert_eq!(engine.total_entries(), 1);
        assert_eq!(engine.get(b"key_alpha").unwrap(), Some(b"v2".to_vec()));

        // 3: Growth overwrite
        let large_val = vec![0x5A; 0x80];
        let _ = engine.set(b"key_alpha", &large_val).unwrap();
        assert_eq!(engine.total_entries(), 1);
        assert_eq!(engine.get(b"key_alpha").unwrap(), Some(large_val));

        // 4: Delete key
        assert!(engine.del(b"key_alpha").unwrap());
        assert_eq!(engine.total_entries(), 0);
        assert!(!engine.contains_key(b"key_alpha").unwrap());
        assert_eq!(engine.get(b"key_alpha").unwrap(), None);
        assert!(!engine.del(b"key_alpha").unwrap());

        // 5: Re-insert after tombstone
        let _ = engine.set(b"key_alpha", b"revived").unwrap();
        assert_eq!(engine.total_entries(), 1);
        assert_eq!(engine.get(b"key_alpha").unwrap(), Some(b"revived".to_vec()));

        engine.delete().unwrap();
    }

    #[test]
    fn err_engine_kv_boundaries_and_type_mismatch() {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path());
        let engine = Engine::create(cfg).unwrap();

        // 1: Empty key boundary
        assert_eq!(engine.get(b"").unwrap(), None);
        assert!(!engine.contains_key(b"").unwrap());
        assert!(!engine.del(b"").unwrap());
        assert!(matches!(engine.set(b"", b"val"), Err(TurboError::PayloadTooLarge)));

        // 2: Oversized key
        let long_key = vec![0x41; MAX_KEY_LEN + 1];
        assert_eq!(engine.get(&long_key).unwrap(), None);
        assert!(!engine.contains_key(&long_key).unwrap());
        assert!(!engine.del(&long_key).unwrap());
        assert!(matches!(engine.set(&long_key, b"val"), Err(TurboError::PayloadTooLarge)));

        // 3: Oversized value
        let long_val = vec![0x42; MAX_VAL_LEN + 1];
        assert!(matches!(engine.set(b"k", &long_val), Err(TurboError::PayloadTooLarge)));

        // 4: Independent coexistence of identical keys across KV and list keyspaces
        let _ = engine.list_append(b"shared_k", b"elem").unwrap();
        assert_eq!(engine.get(b"shared_k").unwrap(), None);
        assert!(!engine.contains_key(b"shared_k").unwrap());
        assert!(engine.set(b"shared_k", b"val").is_ok());
        assert_eq!(engine.get(b"shared_k").unwrap(), Some(b"val".to_vec()));
        assert_eq!(engine.list_len(b"shared_k").unwrap(), 1);

        engine.delete().unwrap();
    }

    #[test]
    fn ok_engine_list_append_pop_and_indexing() {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path());
        let engine = Engine::create(cfg).unwrap();

        // 1: Empty list queries
        assert_eq!(engine.list_len(b"target_list").unwrap(), 0);
        assert_eq!(engine.list_first(b"target_list").unwrap(), None);
        assert_eq!(engine.list_last(b"target_list").unwrap(), None);
        assert_eq!(engine.list_get_at(b"target_list", 0).unwrap(), None);
        assert!(engine.list_items(b"target_list").unwrap().is_empty());
        assert_eq!(engine.list_pop(b"target_list").unwrap(), None);

        // 2: Sequential append
        let _ = engine.list_append(b"target_list", b"elem_0").unwrap();
        let _ = engine.list_append(b"target_list", b"elem_1").unwrap();
        let _ = engine.list_append(b"target_list", b"elem_2").unwrap();

        assert_eq!(engine.list_len(b"target_list").unwrap(), 3);
        assert_eq!(engine.list_first(b"target_list").unwrap(), Some(b"elem_0".to_vec()));
        assert_eq!(engine.list_last(b"target_list").unwrap(), Some(b"elem_2".to_vec()));
        assert_eq!(engine.list_get_at(b"target_list", 0).unwrap(), Some(b"elem_0".to_vec()));
        assert_eq!(engine.list_get_at(b"target_list", 1).unwrap(), Some(b"elem_1".to_vec()));
        assert_eq!(engine.list_get_at(b"target_list", 2).unwrap(), Some(b"elem_2".to_vec()));
        assert_eq!(engine.list_get_at(b"target_list", 3).unwrap(), None);

        assert_eq!(
            engine.list_items(b"target_list").unwrap(),
            vec![b"elem_0".to_vec(), b"elem_1".to_vec(), b"elem_2".to_vec()]
        );

        // 3: Pop stack order
        assert_eq!(engine.list_pop(b"target_list").unwrap(), Some(b"elem_2".to_vec()));
        assert_eq!(engine.list_len(b"target_list").unwrap(), 2);
        assert_eq!(engine.list_pop(b"target_list").unwrap(), Some(b"elem_1".to_vec()));
        assert_eq!(engine.list_len(b"target_list").unwrap(), 1);
        assert_eq!(engine.list_pop(b"target_list").unwrap(), Some(b"elem_0".to_vec()));
        assert_eq!(engine.list_len(b"target_list").unwrap(), 0);
        assert_eq!(engine.list_pop(b"target_list").unwrap(), None);

        engine.delete().unwrap();
    }

    #[test]
    fn ok_engine_list_array_chaining_overflow() {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path());
        let engine = Engine::create(cfg).unwrap();

        for i in 0..0x14 {
            let item_payload = format!("chained_elem_{i}");
            let _ = engine.list_append(b"chain_lst", item_payload.as_bytes()).unwrap();
        }

        assert_eq!(engine.list_len(b"chain_lst").unwrap(), 0x14);
        assert_eq!(engine.list_first(b"chain_lst").unwrap(), Some(b"chained_elem_0".to_vec()));
        assert_eq!(engine.list_last(b"chain_lst").unwrap(), Some(b"chained_elem_19".to_vec()));

        assert_eq!(
            engine.list_get_at(b"chain_lst", 0x0D).unwrap(),
            Some(b"chained_elem_13".to_vec())
        );
        assert_eq!(
            engine.list_get_at(b"chain_lst", 0x0E).unwrap(),
            Some(b"chained_elem_14".to_vec())
        );
        assert_eq!(engine.list_get_at(b"chain_lst", 0x14).unwrap(), None);

        for i in (0..0x14).rev() {
            let expected = format!("chained_elem_{i}").into_bytes();
            assert_eq!(engine.list_pop(b"chain_lst").unwrap(), Some(expected));
        }

        assert_eq!(engine.list_len(b"chain_lst").unwrap(), 0);
        assert_eq!(engine.list_pop(b"chain_lst").unwrap(), None);

        engine.delete().unwrap();
    }

    #[test]
    fn ok_engine_list_deletion_cleanup() {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path());
        let engine = Engine::create(cfg).unwrap();

        for i in 0..5 {
            let _ = engine.list_append(b"clean_lst", format!("item_{i}").as_bytes()).unwrap();
        }

        assert!(engine.list_del(b"clean_lst").unwrap());
        assert_eq!(engine.list_len(b"clean_lst").unwrap(), 0);
        assert!(engine.list_items(b"clean_lst").unwrap().is_empty());
        assert!(!engine.list_del(b"clean_lst").unwrap());

        let _ = engine.list_append(b"clean_lst2", b"v").unwrap();
        assert!(engine.list_del(b"clean_lst2").unwrap());
        assert_eq!(engine.list_len(b"clean_lst2").unwrap(), 0);
        assert!(!engine.list_del(b"clean_lst2").unwrap());

        engine.delete().unwrap();
    }

    #[test]
    fn ok_engine_list_spanned_operations() {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path());
        let engine = Engine::create(cfg).unwrap();

        // 255-byte key (spans across 2 DBufs)
        let long_key = vec![0x37; MAX_KEY_LEN];
        // 1024-byte item (spans across multiple DBufs)
        let large_item1 = vec![0xAA; 1024];
        let large_item2 = vec![0xBB; 2048];

        let _ = engine.list_append(&long_key, &large_item1).unwrap();
        let _ = engine.list_append(&long_key, &large_item2).unwrap();

        assert_eq!(engine.list_len(&long_key).unwrap(), 2);
        assert_eq!(engine.list_first(&long_key).unwrap(), Some(large_item1.clone()));
        assert_eq!(engine.list_last(&long_key).unwrap(), Some(large_item2.clone()));
        assert_eq!(engine.list_get_at(&long_key, 0).unwrap(), Some(large_item1.clone()));
        assert_eq!(engine.list_get_at(&long_key, 1).unwrap(), Some(large_item2.clone()));

        let items = engine.list_items(&long_key).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], large_item1);
        assert_eq!(items[1], large_item2);

        // Pop last item
        assert_eq!(engine.list_pop(&long_key).unwrap(), Some(large_item2));
        assert_eq!(engine.list_len(&long_key).unwrap(), 1);

        // Delete list and verify cleanup
        assert!(engine.list_del(&long_key).unwrap());
        assert_eq!(engine.list_len(&long_key).unwrap(), 0);
        assert_eq!(engine.list_first(&long_key).unwrap(), None);

        engine.delete().unwrap();
    }

    #[test]
    fn ok_engine_trie_split_growth() {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path());
        let engine = Engine::create(cfg).unwrap();

        // Inserting 50 keys will force multiple bucket splits across 30-way trie
        for i in 0..0x32 {
            let k = format!("trie_key_{i}").into_bytes();
            let v = format!("trie_val_{i}").into_bytes();
            let ticket = engine.set(&k, &v).unwrap();
            assert!(ticket.epoch() > 0);
        }

        assert_eq!(engine.total_entries(), 0x32);
        for i in 0..0x32 {
            let k = format!("trie_key_{i}").into_bytes();
            let expected = format!("trie_val_{i}").into_bytes();
            assert_eq!(engine.get(&k).unwrap(), Some(expected));
        }

        engine.delete().unwrap();
    }

    #[test]
    fn err_engine_list_boundaries_and_type_mismatch() {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path());
        let engine = Engine::create(cfg).unwrap();

        assert!(matches!(engine.list_append(b"", b"item"), Err(TurboError::PayloadTooLarge)));
        assert_eq!(engine.list_pop(b"").unwrap(), None);
        assert_eq!(engine.list_first(b"").unwrap(), None);
        assert_eq!(engine.list_last(b"").unwrap(), None);
        assert_eq!(engine.list_get_at(b"", 0).unwrap(), None);
        assert_eq!(engine.list_len(b"").unwrap(), 0);
        assert!(engine.list_items(b"").unwrap().is_empty());
        assert!(!engine.list_del(b"").unwrap());

        let long_key = vec![0x41; MAX_KEY_LEN + 1];
        assert!(matches!(engine.list_append(&long_key, b"item"), Err(TurboError::PayloadTooLarge)));
        let long_val = vec![0x42; MAX_VAL_LEN + 1];
        assert!(matches!(engine.list_append(b"k", &long_val), Err(TurboError::PayloadTooLarge)));

        let _ = engine.set(b"kv_entry_key", b"scalar_val").unwrap();
        assert!(engine.list_append(b"kv_entry_key", b"item").is_ok());
        assert_eq!(engine.get(b"kv_entry_key").unwrap(), Some(b"scalar_val".to_vec()));
        assert_eq!(engine.list_len(b"kv_entry_key").unwrap(), 1);

        engine.delete().unwrap();
    }

    #[test]
    fn ok_engine_metrics_and_sync() {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path());
        let engine = Engine::create(cfg).unwrap();

        assert_eq!(engine.total_entries(), 0);
        let resident_bytes = engine.resident_bytes_in_memory().unwrap();
        assert!(resident_bytes >= engine.sys_page_size.as_usize());
        assert_eq!(engine.dbuf_slot_cache.as_ref().map_or(0, |c| c.memory_bytes()), 0);

        let cached_dir = tempdir().unwrap();
        let cached_cfg = TurboCfg::manual(cached_dir.path()).with_lookup_cache(true);
        let cached_engine = Engine::create(cached_cfg).unwrap();
        let cached_resident = cached_engine.resident_bytes_in_memory().unwrap();
        assert!(cached_resident >= core::mem::size_of::<DBufSlotCache>());
        cached_engine.delete().unwrap();

        let resident_disk = engine.resident_bytes_on_disk();
        assert!(resident_disk >= engine.sys_page_size.as_usize());

        let _ = engine.set(b"k1", b"v1").unwrap();
        let _ = engine.set(b"k2", b"v2").unwrap();
        assert_eq!(engine.total_entries(), 2);

        engine.sync().unwrap();
        engine.delete().unwrap();
    }

    #[test]
    fn ok_engine_eviction_modes() {
        for &lookup_cache in &[false, true] {
            let dir = tempdir().unwrap();
            let cfg = TurboCfg::manual(dir.path()).with_lookup_cache(lookup_cache);
            let engine = Engine::create(cfg).unwrap();

            for i in 0..0x80 {
                let k = format!("k_{:04x}", i);
                let v = format!("val_{:04x}", i);
                let _ = engine.set(k.as_bytes(), v.as_bytes()).unwrap();
            }

            engine.sync().unwrap();
            let pre_resident = engine.resident_bytes_in_memory().unwrap();
            assert!(pre_resident > 0);

            // 1: EvictionMode::PreserveHeaders
            engine.evict(EvictionMode::PreserveHeaders).unwrap();
            for i in 0..0x80 {
                let k = format!("k_{:04x}", i);
                let expected = format!("val_{:04x}", i);
                assert_eq!(engine.get(k.as_bytes()).unwrap(), Some(expected.into_bytes()));
            }

            // 2: EvictionMode::PurgeAll
            engine.evict(EvictionMode::PurgeAll).unwrap();
            for i in 0..0x80 {
                let k = format!("k_{:04x}", i);
                let expected = format!("val_{:04x}", i);
                assert_eq!(engine.get(k.as_bytes()).unwrap(), Some(expected.into_bytes()));
            }

            engine.delete().unwrap();
        }
    }

    #[test]
    fn ok_engine_jitter_and_occ_backoff() {
        for attempt in 0..0x10 {
            let j = Engine::fast_jitter(attempt);
            assert!((0x4B..=0x7E).contains(&j));
        }

        Engine::occ_backoff(0);
        Engine::occ_backoff(4);
        Engine::occ_backoff(8);
    }
}
