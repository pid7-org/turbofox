use crate::adjarr::{AdjArrayBlock, decompose_relative_array_idx, relative_array_idx};
use crate::bucket::{Bucket, BucketEntry, ProbeResult};
use crate::cfg::TurboCfg;
use crate::common::{
    ADJ_ARR_HEAD_SLOT, ADJ_ARR_SLOTS, ALLOC_SLOT_QUANTUM, BUCKET_ENTRIES, BUCKET_FLAG,
    BUCKET_SLOTS, DATABUF_HEAD_SLOT, DB_NAME_TAG, DBUF_BUF_COUNT, DBUF_SLOTS,
    EXTRA_NODE_START_SLOT, INITIAL_BUCKET_COUNT, INITIAL_BUCKET_START_SLOT, INITIAL_DATABUF_POOLS,
    INITIAL_EXTRA_NODES, INITIAL_FREE_SLOT_MARKER, LAYOUT_VERSION, MAGIC, MAX_DB_LEN, MAX_KEY_LEN,
    MAX_TRIE_DEPTH, MAX_VAL_LEN, MODULE_ID, NODE_CHILDREN, NODE_FLAG, OCC_MAX_RETRIES,
    ROOT_NODE_SLOT, SLOT_SIZE, Slot,
};
use crate::dbuf::DBuf;
use crate::error::{TurboError, TurboResult};
use crate::hash::{hash_key, trie_child_idx};
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
}

impl Drop for DBufSlotCache {
    fn drop(&mut self) {
        for chunk in &self.chunks {
            let ptr = chunk.load(Ordering::Relaxed);
            if !ptr.is_null() {
                unsafe {
                    drop(Box::from_raw(ptr));
                }
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

#[derive(Debug)]
pub(crate) struct Engine {
    mmap: MemMap<Slot>,
    growth_mutex: Mutex<()>,
    total_slots: AtomicU32,
    total_entries: AtomicU64,
    free_slot_marker: AtomicU32,
    active_dbuf_hint: AtomicU32,
    sys_page_size: PageSize,
    dbuf_slot_cache: DBufSlotCache,
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
            sync_interval: cfg.sync_interval(),
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
            active_dbuf_hint: AtomicU32::new(0),
            sys_page_size: page_size,
            dbuf_slot_cache: DBufSlotCache::new(),
        };

        engine.format_initial_topography(LAYOUT_VERSION, initial_pages as u32)?;
        engine.dbuf_slot_cache.push(0, DATABUF_HEAD_SLOT);
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
            sequential_access_hint: true,
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
        if root_flag != NODE_FLAG {
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
            dbuf_slot_cache: DBufSlotCache::new(),
        };

        engine.dbuf_slot_cache.push(0, DATABUF_HEAD_SLOT);

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
            self.grow_backing_file(count)?;
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

    fn expand_databuf_locked(&self, tail_slot: usize) -> TurboResult<u32> {
        let pool_count = 4usize;
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
        for i in 0..pool_count {
            let s = new_slot + (i * DBUF_SLOTS) as u32;
            let hop = tail_hop + 1 + (i as u32);
            self.dbuf_slot_cache.push(hop, s);
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
        if let Some(slot) = self.dbuf_slot_cache.get(pool_hop) {
            return Ok(slot);
        }

        let mut start_hop = 0u32;
        let mut curr = DATABUF_HEAD_SLOT;
        for h in (1..pool_hop).rev() {
            if let Some(slot) = self.dbuf_slot_cache.get(h) {
                start_hop = h;
                curr = slot;
                break;
            }
        }

        for h in (start_hop + 1)..=pool_hop {
            let next =
                self.mmap.read_exact::<DBuf, _, _>(curr as usize, |d| d.next_dbuf_index())?;

            if next == 0 {
                return Err(TurboError::CorruptedHeader("broken databuf chain"));
            }

            curr = next;
            self.dbuf_slot_cache.push(h, curr);
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
        let hint_hop = self.active_dbuf_hint.load(Ordering::Relaxed);
        let start_slot = self.locate_databuf_slot(hint_hop).unwrap_or(DATABUF_HEAD_SLOT) as usize;
        let start_hop = if start_slot == (DATABUF_HEAD_SLOT as usize) { 0 } else { hint_hop };

        let mut curr = start_slot;
        let mut curr_hop = start_hop;
        let mut tail_slot = start_slot;
        let mut tail_hop = start_hop;
        let mut wrapped = false;

        loop {
            if curr == exclude_slot {
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
                break;
            }

            actual_tail = next;
            actual_tail_hop += 1;
            if actual_tail != exclude_slot {
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

        let new_slot = self.expand_databuf_locked(actual_tail)? as usize;
        let new_hop = actual_tail_hop + 1;
        self.active_dbuf_hint.store(new_hop, Ordering::Relaxed);

        let mut dbuf = self.mmap.read_exact::<DBuf, _, _>(new_slot, |d| *d)?;
        let orig = dbuf;
        let local_idx = dbuf.alloc_contiguous(needed).ok_or(TurboError::DatabaseFull)?;

        Ok(DbufAllocPlan {
            start_slot: new_slot,
            pool_hop: new_hop,
            local_idx,
            orig,
            mutated: dbuf,
        })
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
            let flag = self.mmap.read_exact::<Slot, _, _>(target_slot as usize, |s| s.flag())?;

            if flag == NODE_FLAG {
                curr_slot = target_slot;
                depth += 1;
            } else if flag == BUCKET_FLAG {
                return Ok((curr_slot, child_idx, target_slot, depth));
            } else {
                return Err(TurboError::InvalidRootNode);
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

        Ok((new_node_slot, compacted_bucket, mutated_parent))
    }

    /// Ref: `DESIGN.md` §7.1
    pub(crate) fn get(&self, key: &[u8]) -> TurboResult<Option<Vec<u8>>> {
        if key.is_empty() || key.len() > MAX_KEY_LEN {
            return Ok(None);
        }

        let hash = hash_key(key);
        let (_, _, bucket_slot, _depth) = self.traverse_to_leaf(hash)?;
        let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;

        let matched_idx =
            bucket.probe_match(hash, |entry_idx| self.match_any_key(&bucket.entry(entry_idx), key));

        match matched_idx {
            Some(entry_idx) => {
                let entry = bucket.entry(entry_idx);
                if entry.is_list() {
                    return Err(TurboError::InvalidType("Key is a list"));
                }

                let (pool_hop, local_idx) = DBuf::decompose_buf_idx(entry.buf_idx());
                let dbuf_slot = self.locate_databuf_slot(pool_hop)?;
                let dbuf = self.mmap.read_exact::<DBuf, _, _>(dbuf_slot as usize, |d| *d)?;

                let (_, val) = dbuf
                    .read_kv_payload(local_idx, entry.klen() as usize, entry.vlen() as usize)
                    .map_err(|_| TurboError::CorruptedPayload("KV CRC32 mismatch"))?;

                Ok(Some(val.to_vec()))
            }
            None => Ok(None),
        }
    }

    pub(crate) fn contains_key(&self, key: &[u8]) -> TurboResult<bool> {
        if key.is_empty() || key.len() > MAX_KEY_LEN {
            return Ok(false);
        }

        let hash = hash_key(key);
        let (_, _, bucket_slot, _depth) = self.traverse_to_leaf(hash)?;
        let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;

        let matched_idx =
            bucket.probe_match(hash, |entry_idx| self.match_any_key(&bucket.entry(entry_idx), key));

        match matched_idx {
            Some(entry_idx) => {
                let entry = bucket.entry(entry_idx);
                if entry.is_list() {
                    return Err(TurboError::InvalidType("Key is a list"));
                }

                Ok(true)
            }
            None => Ok(false),
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

        let (pool_hop, local_idx) = DBuf::decompose_buf_idx(entry.buf_idx());
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
    }

    /// Ref: `DESIGN.md` §7.2
    pub(crate) fn set(&self, key: &[u8], val: &[u8]) -> TurboResult<TurboTicket> {
        if key.is_empty() || key.len() > MAX_KEY_LEN || val.len() > MAX_VAL_LEN {
            return Err(TurboError::PayloadTooLarge);
        }

        let req = DBuf::required_kv_buffers(key.len(), val.len());
        if req > DBUF_BUF_COUNT {
            return Err(TurboError::PayloadTooLarge);
        }

        let hash = hash_key(key);

        for attempt in 0..OCC_MAX_RETRIES {
            let (parent_slot, parent_child_idx, bucket_slot, depth) =
                self.traverse_to_leaf(hash)?;
            let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;

            let probe_res =
                bucket.probe(hash, |entry_idx| self.match_any_key(&bucket.entry(entry_idx), key));

            match probe_res {
                ProbeResult::Found(target_entry_idx) => {
                    let old_entry = bucket.entry(target_entry_idx);
                    if old_entry.is_list() {
                        return Err(TurboError::InvalidType("cannot overwrite list key with KV"));
                    }

                    let old_req = DBuf::required_kv_buffers(
                        old_entry.klen() as usize,
                        old_entry.vlen() as usize,
                    );

                    let (old_pool_hop, old_local_idx) =
                        DBuf::decompose_buf_idx(old_entry.buf_idx());

                    let old_dbuf_slot = self.locate_databuf_slot(old_pool_hop)? as usize;
                    let orig_old_dbuf =
                        self.mmap.read_exact::<DBuf, _, _>(old_dbuf_slot, |d| *d)?;

                    let target_global_buf: u32;
                    let mut touched_dbufs: Vec<(usize, DBuf, DBuf)> = Vec::with_capacity(2);

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

                            target_global_buf = DBuf::global_buf_idx(old_pool_hop, new_local);
                            touched_dbufs.push((old_dbuf_slot, orig_old_dbuf, freed_old));
                        } else {
                            let mut plan = self.find_free_databuf(req, old_dbuf_slot)?;
                            plan.mutated
                                .write_kv_payload(plan.local_idx, key, val)
                                .map_err(|_| TurboError::PayloadTooLarge)?;

                            target_global_buf = DBuf::global_buf_idx(plan.pool_hop, plan.local_idx);

                            touched_dbufs.push((old_dbuf_slot, orig_old_dbuf, freed_old));
                            touched_dbufs.push((plan.start_slot, plan.orig, plan.mutated));
                        }
                    }

                    let mut new_bucket = bucket;
                    new_bucket.set_kv(
                        target_entry_idx,
                        hash,
                        key.len() as u8,
                        val.len() as u16,
                        target_global_buf,
                    );

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
                    let mut plan = self.find_free_databuf(req, 0)?;
                    plan.mutated
                        .write_kv_payload(plan.local_idx, key, val)
                        .map_err(|_| TurboError::PayloadTooLarge)?;

                    let target_global_buf = DBuf::global_buf_idx(plan.pool_hop, plan.local_idx);

                    let mut new_bucket = bucket;
                    new_bucket.set_kv(
                        target_entry_idx,
                        hash,
                        key.len() as u8,
                        val.len() as u16,
                        target_global_buf,
                    );

                    let mut changes: Vec<(usize, Slot, Slot)> =
                        Vec::with_capacity(BUCKET_SLOTS + DBUF_SLOTS);

                    for i in 0..BUCKET_SLOTS {
                        let orig_s = bucket.as_slots()[i];
                        let new_s = new_bucket.as_slots()[i];

                        if orig_s != new_s {
                            changes.push(((bucket_slot as usize) + i, orig_s, new_s));
                        }
                    }

                    for i in 0..DBUF_SLOTS {
                        let orig_s = plan.orig.as_slots()[i];
                        let new_s = plan.mutated.as_slots()[i];

                        if orig_s != new_s {
                            changes.push((plan.start_slot + i, orig_s, new_s));
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

                    let mut plan = self.find_free_databuf(req, 0)?;
                    plan.mutated
                        .write_kv_payload(plan.local_idx, key, val)
                        .map_err(|_| TurboError::PayloadTooLarge)?;

                    let target_global_buf = DBuf::global_buf_idx(plan.pool_hop, plan.local_idx);

                    let new_entry = BucketEntry::pack_kv(
                        hash,
                        key.len() as u8,
                        val.len() as u16,
                        target_global_buf,
                    );

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
                        Vec::with_capacity(1 + BUCKET_SLOTS + DBUF_SLOTS);

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

                    for i in 0..DBUF_SLOTS {
                        let orig_s = plan.orig.as_slots()[i];
                        let new_s = plan.mutated.as_slots()[i];

                        if orig_s != new_s {
                            changes.push((plan.start_slot + i, orig_s, new_s));
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

        let hash = hash_key(key);

        for attempt in 0..OCC_MAX_RETRIES {
            let (_, _, bucket_slot, _depth) = self.traverse_to_leaf(hash)?;
            let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;

            let matched_idx = bucket
                .probe_match(hash, |entry_idx| self.match_any_key(&bucket.entry(entry_idx), key));

            match matched_idx {
                Some(target_entry_idx) => {
                    let entry = bucket.entry(target_entry_idx);
                    if entry.is_list() {
                        return self.list_del(key);
                    }

                    let req =
                        DBuf::required_kv_buffers(entry.klen() as usize, entry.vlen() as usize);

                    let (pool_hop, local_idx) = DBuf::decompose_buf_idx(entry.buf_idx());
                    let dbuf_slot = self.locate_databuf_slot(pool_hop)? as usize;
                    let orig_dbuf = self.mmap.read_exact::<DBuf, _, _>(dbuf_slot, |d| *d)?;

                    let mut freed_dbuf = orig_dbuf;
                    freed_dbuf.free_contiguous(local_idx, req);

                    let mut new_bucket = bucket;
                    new_bucket.mark_tombstone(target_entry_idx);

                    let mut changes: Vec<(usize, Slot, Slot)> =
                        Vec::with_capacity(BUCKET_SLOTS + DBUF_SLOTS);

                    for i in 0..BUCKET_SLOTS {
                        let orig_s = bucket.as_slots()[i];
                        let new_s = new_bucket.as_slots()[i];

                        if orig_s != new_s {
                            changes.push(((bucket_slot as usize) + i, orig_s, new_s));
                        }
                    }

                    for i in 0..DBUF_SLOTS {
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
        if req_item > DBUF_BUF_COUNT {
            return Err(TurboError::PayloadTooLarge);
        }

        let hash = hash_key(key);

        for attempt in 0..OCC_MAX_RETRIES {
            let (parent_slot, parent_child_idx, bucket_slot, depth) =
                self.traverse_to_leaf(hash)?;
            let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;

            let probe_res =
                bucket.probe(hash, |entry_idx| self.match_any_key(&bucket.entry(entry_idx), key));

            match probe_res {
                ProbeResult::Found(target_entry_idx) => {
                    let entry = bucket.entry(target_entry_idx);
                    if !entry.is_list() {
                        return Err(TurboError::InvalidType("Key is a KV entry"));
                    }

                    let mut item_plan = self.find_free_databuf(req_item, 0)?;
                    item_plan
                        .mutated
                        .write_list_val_payload(item_plan.local_idx, item)
                        .map_err(|_| TurboError::PayloadTooLarge)?;

                    let item_buf_idx =
                        DBuf::global_buf_idx(item_plan.pool_hop, item_plan.local_idx);

                    let mut curr_arr_idx = entry.adjarr_idx();
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

                    if let Some((_, prev_block_slot, prev_orig_block, _)) = prev_opt {
                        if prev_block_slot != last_block_slot
                            && !touched_blocks.iter().any(|(s, _, _)| *s == prev_block_slot)
                        {
                            touched_blocks.push((
                                prev_block_slot,
                                prev_orig_block,
                                prev_orig_block,
                            ));
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

                    for i in 0..DBUF_SLOTS {
                        let orig_s = item_plan.orig.as_slots()[i];
                        let new_s = item_plan.mutated.as_slots()[i];

                        if orig_s != new_s {
                            changes.push((item_plan.start_slot + i, orig_s, new_s));
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

                    let mut key_plan = self.find_free_databuf(req_key, 0)?;
                    key_plan
                        .mutated
                        .write_list_key_payload(key_plan.local_idx, key)
                        .map_err(|_| TurboError::PayloadTooLarge)?;

                    let key_buf_idx = DBuf::global_buf_idx(key_plan.pool_hop, key_plan.local_idx);

                    let mut item_plan = self.find_free_databuf(req_item, key_plan.start_slot)?;
                    item_plan
                        .mutated
                        .write_list_val_payload(item_plan.local_idx, item)
                        .map_err(|_| TurboError::PayloadTooLarge)?;

                    let item_buf_idx =
                        DBuf::global_buf_idx(item_plan.pool_hop, item_plan.local_idx);

                    let (arr_idx, block_slot, orig_block, mut mut_block) =
                        self.find_free_adjarr_array(0)?;

                    let (_, local_arr) = decompose_relative_array_idx(arr_idx);
                    let _ = mut_block.array_mut(local_arr).push(item_buf_idx);

                    let mut new_bucket = bucket;
                    new_bucket.set_list(target_entry_idx, hash, arr_idx, key_buf_idx);

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

                    if key_plan.start_slot == item_plan.start_slot {
                        for i in 0..DBUF_SLOTS {
                            let orig_s = key_plan.orig.as_slots()[i];
                            let new_s = item_plan.mutated.as_slots()[i];

                            if orig_s != new_s {
                                changes.push((key_plan.start_slot + i, orig_s, new_s));
                            }
                        }
                    } else {
                        for i in 0..DBUF_SLOTS {
                            let orig_s = key_plan.orig.as_slots()[i];
                            let new_s = key_plan.mutated.as_slots()[i];

                            if orig_s != new_s {
                                changes.push((key_plan.start_slot + i, orig_s, new_s));
                            }
                        }

                        for i in 0..DBUF_SLOTS {
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

                    let mut key_plan = self.find_free_databuf(req_key, 0)?;
                    key_plan
                        .mutated
                        .write_list_key_payload(key_plan.local_idx, key)
                        .map_err(|_| TurboError::PayloadTooLarge)?;

                    let key_buf_idx = DBuf::global_buf_idx(key_plan.pool_hop, key_plan.local_idx);

                    let mut item_plan = self.find_free_databuf(req_item, key_plan.start_slot)?;
                    item_plan
                        .mutated
                        .write_list_val_payload(item_plan.local_idx, item)
                        .map_err(|_| TurboError::PayloadTooLarge)?;

                    let item_buf_idx =
                        DBuf::global_buf_idx(item_plan.pool_hop, item_plan.local_idx);

                    let (arr_idx, block_slot, orig_block, mut mut_block) =
                        self.find_free_adjarr_array(0)?;

                    let (_, local_arr) = decompose_relative_array_idx(arr_idx);
                    let _ = mut_block.array_mut(local_arr).push(item_buf_idx);

                    let new_entry = BucketEntry::pack_list(hash, arr_idx, key_buf_idx);

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

                    if key_plan.start_slot == item_plan.start_slot {
                        for i in 0..DBUF_SLOTS {
                            let orig_s = key_plan.orig.as_slots()[i];
                            let new_s = item_plan.mutated.as_slots()[i];

                            if orig_s != new_s {
                                changes.push((key_plan.start_slot + i, orig_s, new_s));
                            }
                        }
                    } else {
                        for i in 0..DBUF_SLOTS {
                            let orig_s = key_plan.orig.as_slots()[i];
                            let new_s = key_plan.mutated.as_slots()[i];

                            if orig_s != new_s {
                                changes.push((key_plan.start_slot + i, orig_s, new_s));
                            }
                        }

                        for i in 0..DBUF_SLOTS {
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
        let Ok(dbuf_slot) = self.locate_databuf_slot(pool_hop) else {
            return false;
        };

        let Ok(matches) = self
            .mmap
            .read_exact::<DBuf, _, _>(dbuf_slot as usize, |d| d.verify_list_key(local_idx, key))
        else {
            return false;
        };

        matches
    }

    /// Ref: `DESIGN.md` §7.4
    pub(crate) fn list_pop(&self, key: &[u8]) -> TurboResult<Option<Vec<u8>>> {
        if key.is_empty() || key.len() > MAX_KEY_LEN {
            return Ok(None);
        }

        let hash = hash_key(key);

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
            let mut curr_arr_idx = entry.adjarr_idx();
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

            let val_bytes = orig_item_dbuf
                .read_list_val_payload(item_local_idx, vlen)
                .map_err(|_| TurboError::CorruptedPayload("List item CRC mismatch"))?
                .to_vec();

            let req_item = DBuf::required_list_val_buffers(vlen);
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
                if let Some((_, prev_block_slot, prev_orig_block, _)) = prev_opt {
                    if prev_block_slot != last_block_slot
                        && !touched_blocks.iter().any(|(s, _, _)| *s == prev_block_slot)
                    {
                        touched_blocks.push((prev_block_slot, prev_orig_block, prev_orig_block));
                    }
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

            for i in 0..DBUF_SLOTS {
                let orig_s = orig_item_dbuf.as_slots()[i];
                let new_s = freed_item_dbuf.as_slots()[i];

                if orig_s != new_s {
                    changes.push((item_dbuf_slot + i, orig_s, new_s));
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

        let hash = hash_key(key);
        let (_, _, bucket_slot, _depth) = self.traverse_to_leaf(hash)?;

        let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;
        let matched_idx = bucket
            .probe_match(hash, |entry_idx| self.match_list_key(&bucket.entry(entry_idx), key));

        let target_entry_idx = match matched_idx {
            Some(idx) => idx,
            None => return Ok(None),
        };

        let entry = bucket.entry(target_entry_idx);
        let (block_hop, local_arr) = decompose_relative_array_idx(entry.adjarr_idx());

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

        let hash = hash_key(key);
        let (_, _, bucket_slot, _depth) = self.traverse_to_leaf(hash)?;
        let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;

        let matched_idx = bucket
            .probe_match(hash, |entry_idx| self.match_list_key(&bucket.entry(entry_idx), key));

        let target_entry_idx = match matched_idx {
            Some(idx) => idx,
            None => return Ok(None),
        };

        let entry = bucket.entry(target_entry_idx);
        let mut curr_arr_idx = entry.adjarr_idx();
        let mut last_occupied_buf_idx: Option<u32> = None;

        loop {
            let (block_hop, local_arr) = decompose_relative_array_idx(curr_arr_idx);
            let block_slot = self.locate_adjarr_block_slot(block_hop)? as usize;
            let block = self.mmap.read_exact::<AdjArrayBlock, _, _>(block_slot, |b| *b)?;

            let array = block.array(local_arr);
            for i in (0..array.len()).rev() {
                if let Some(e) = array.entry(i) {
                    if !e.is_empty() {
                        last_occupied_buf_idx = Some(e.buf_idx());
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
            let mut curr_arr_idx = entry.adjarr_idx();
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

        let hash = hash_key(key);

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
            let mut curr_arr_idx = entry.adjarr_idx();
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

        let hash = hash_key(key);

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
            let mut curr_arr_idx = entry.adjarr_idx();
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
                    if let Some(e) = array.entry(i) {
                        if !e.is_empty() {
                            match self.read_list_item_payload(e.buf_idx()) {
                                Ok(payload) => items.push(payload),
                                Err(_) => {
                                    retry_needed = true;
                                    break;
                                }
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

        let hash = hash_key(key);

        for attempt in 0..OCC_MAX_RETRIES {
            let (_, _, bucket_slot, _depth) = self.traverse_to_leaf(hash)?;
            let bucket = self.mmap.read_exact::<Bucket, _, _>(bucket_slot as usize, |b| *b)?;

            let matched_idx = bucket
                .probe_match(hash, |entry_idx| self.match_list_key(&bucket.entry(entry_idx), key));

            match matched_idx {
                Some(target_entry_idx) => {
                    let entry = bucket.entry(target_entry_idx);

                    let (key_pool_hop, key_local_idx) = DBuf::decompose_buf_idx(entry.buf_idx());
                    let key_dbuf_slot = self.locate_databuf_slot(key_pool_hop)? as usize;

                    let orig_key_dbuf =
                        self.mmap.read_exact::<DBuf, _, _>(key_dbuf_slot, |d| *d)?;

                    let req_key = DBuf::required_list_key_buffers(key.len());

                    let mut freed_key_dbuf = orig_key_dbuf;
                    freed_key_dbuf.free_contiguous(key_local_idx, req_key);

                    let mut curr_arr_idx = entry.adjarr_idx();
                    let mut touched_blocks: Vec<(usize, AdjArrayBlock, AdjArrayBlock)> = Vec::new();
                    let mut touched_item_dbufs: std::collections::HashMap<usize, (DBuf, DBuf)> =
                        std::collections::HashMap::new();

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
                                let item_dbuf_slot =
                                    self.locate_databuf_slot(item_pool_hop)? as usize;

                                let (_orig_d, mut_d) =
                                    touched_item_dbufs.entry(item_dbuf_slot).or_insert_with(|| {
                                        if item_dbuf_slot == key_dbuf_slot {
                                            (orig_key_dbuf, freed_key_dbuf)
                                        } else {
                                            let d = self
                                                .mmap
                                                .read_exact::<DBuf, _, _>(item_dbuf_slot, |d| *d)
                                                .unwrap();
                                            (d, d)
                                        }
                                    });

                                let vlen = mut_d.read_list_vlen(item_local_idx).unwrap_or(0);
                                let req_item = DBuf::required_list_val_buffers(vlen);
                                mut_d.free_contiguous(item_local_idx, req_item);
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

                    if !touched_item_dbufs.contains_key(&key_dbuf_slot) {
                        for i in 0..DBUF_SLOTS {
                            let orig_s = orig_key_dbuf.as_slots()[i];
                            let new_s = freed_key_dbuf.as_slots()[i];
                            if orig_s != new_s {
                                changes.push((key_dbuf_slot + i, orig_s, new_s));
                            }
                        }
                    }

                    for (dbuf_slot, (orig_d, mut_d)) in &touched_item_dbufs {
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
    pub(crate) fn total_pages(&self) -> TurboResult<u32> {
        let meta = self.mmap.read_exact::<Metadata, _, _>(0, |m| *m)?;
        Ok(meta.total_pages)
    }

    /// Ref: `DESIGN.md` §8.1
    pub(crate) fn remaining_pages(&self) -> TurboResult<u32> {
        let meta = self.mmap.read_exact::<Metadata, _, _>(0, |m| *m)?;
        let used_slots = meta.free_slot_marker;
        let total_slots = meta.total_slots;

        if used_slots >= total_slots {
            return Ok(0);
        }

        let free_slots = total_slots - used_slots;
        let page_slots = meta.page_size / (SLOT_SIZE as u32);

        Ok(free_slots / page_slots)
    }

    /// Ref: `DESIGN.md` §8.1
    pub(crate) fn remaining_bytes(&self) -> TurboResult<u64> {
        let meta = self.mmap.read_exact::<Metadata, _, _>(0, |m| *m)?;
        let used_slots = meta.free_slot_marker as u64;
        let total_slots = meta.total_slots as u64;

        if used_slots >= total_slots {
            return Ok(0);
        }

        Ok((total_slots - used_slots) * (SLOT_SIZE as u64))
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
        assert!(engine.total_pages().unwrap() > 0);

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

        let initial_pages = engine.total_pages().unwrap();
        let initial_slots = engine.total_slots.load(Ordering::Acquire);
        let huge_alloc = {
            let _guard = engine.growth_mutex.lock().unwrap();
            engine.alloc_uniform_slots_locked(initial_slots + 0x40).unwrap()
        };
        assert!(huge_alloc >= initial_free + 0x20);
        assert!(engine.total_pages().unwrap() > initial_pages);
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
            let s = engine.expand_databuf_locked(last_init_slot as usize).unwrap();
            let new_hop = engine.databuf_slot_to_hop(s).unwrap();
            engine.active_dbuf_hint.store(new_hop, Ordering::Relaxed);
            s
        };
        assert_eq!(engine.locate_databuf_slot(INITIAL_DATABUF_POOLS as u32).unwrap(), new_slot);
        assert_eq!(engine.databuf_slot_to_hop(new_slot).unwrap(), INITIAL_DATABUF_POOLS as u32);

        let plan = engine.find_free_databuf(0x04, 0).unwrap();
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

        // 4: Type mismatch on list entry
        let _ = engine.list_append(b"list_item_k", b"elem").unwrap();
        assert!(matches!(engine.get(b"list_item_k"), Err(TurboError::InvalidType(_))));
        assert!(matches!(engine.contains_key(b"list_item_k"), Err(TurboError::InvalidType(_))));
        assert!(matches!(engine.set(b"list_item_k", b"val"), Err(TurboError::InvalidType(_))));

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
        assert!(engine.del(b"clean_lst2").unwrap());
        assert_eq!(engine.list_len(b"clean_lst2").unwrap(), 0);
        assert!(!engine.del(b"clean_lst2").unwrap());

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
        assert!(matches!(
            engine.list_append(b"kv_entry_key", b"item"),
            Err(TurboError::InvalidType(_))
        ));

        engine.delete().unwrap();
    }

    #[test]
    fn ok_engine_metrics_and_sync() {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path());
        let engine = Engine::create(cfg).unwrap();

        assert_eq!(engine.total_entries(), 0);
        assert!(engine.total_pages().unwrap() >= 1);
        if engine.sys_page_size == PageSize::K4 {
            assert!(engine.remaining_pages().unwrap() > 0);
        } else {
            assert_eq!(engine.remaining_pages().unwrap(), 0);
        }
        assert!(engine.remaining_bytes().unwrap() > 0);

        let _ = engine.set(b"k1", b"v1").unwrap();
        let _ = engine.set(b"k2", b"v2").unwrap();
        assert_eq!(engine.total_entries(), 2);

        engine.sync().unwrap();
        engine.delete().unwrap();
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
