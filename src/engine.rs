//! Low-level storage engine managing memory mapping, topography, and OCC transactions

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use frozen_core::memmap::{MemMap, MemMapCfg, PageSize};

use crate::adjarr::AdjArrayBlock;
use crate::bucket::Bucket;
use crate::cfg::{MODULE_ID, PAGE_SIZE, TurboCfg};
use crate::databuf::DataBuf;
use crate::error::{TurboError, TurboResult};
use crate::meta::Metadata;
use crate::node::Node;
use crate::slot::Slot;

const INITIAL_PAGES: u32 = 8;
const INITIAL_SLOTS: usize = 0x0100;
const MIN_FILE_LEN: usize = (INITIAL_PAGES as usize) * (PAGE_SIZE as usize);

const ROOT_NODE_SLOT: usize = (Metadata::DEFAULT_ROOT_NODE_VP as usize) * 2;
const FIRST_BUCKET_VP: u32 = 4;
const LAST_BUCKET_VP: u32 = 0x43;

const DATABUF_SLOTS: [usize; 0x0E] = [
    0x88, 0x90, 0x98, 0xA0, 0xA8, 0xB0, 0xB8,
    0xC8, 0xD0, 0xD8, 0xE0, 0xE8, 0xF0, 0xF8,
];

/// Low-level storage engine coordinating memory mapping and disk topography
#[derive(Debug)]
pub(crate) struct Engine {
    mmap: MemMap<Slot>,
    growth_mutex: Mutex<()>,
    total_slots: AtomicU32,
}

impl Engine {
    /// Opens existing database file or initializes new database layout per DESIGN.md §2
    pub(crate) fn open_or_create(cfg: TurboCfg) -> TurboResult<Self> {
        let vpath = cfg.versioned_path();
        if vpath.exists() {
            Self::open(cfg)
        } else {
            Self::create(cfg)
        }
    }

    /// Initializes a brand new database with 8 physical OS pages formatted in-place
    fn create(cfg: TurboCfg) -> TurboResult<Self> {
        let memmap_cfg = MemMapCfg {
            module_id: MODULE_ID,
            path: cfg.path.clone(),
            version: Metadata::CURRENT_VERSION,
            initial_t: INITIAL_SLOTS,
            sync_interval: cfg.sync_interval,
            page_size: Some(PageSize::K4),
        };

        let mmap = MemMap::<Slot>::new(memmap_cfg)?;
        let engine = Self {
            mmap,
            growth_mutex: Mutex::new(()),
            total_slots: AtomicU32::new(INITIAL_SLOTS as u32),
        };

        engine.format_initial_topography()?;
        engine.sync()?;
        Ok(engine)
    }

    /// Opens an existing database file and verifies all geometric and structural invariants
    fn open(cfg: TurboCfg) -> TurboResult<Self> {
        let vpath = cfg.versioned_path();
        let file_meta = std::fs::metadata(&vpath)?;
        if file_meta.is_dir() {
            return Err(TurboError::Io(std::io::Error::other(
                "target path is a directory",
            )));
        }

        let file_len = file_meta.len() as usize;
        if file_len < MIN_FILE_LEN {
            return Err(TurboError::InvalidSize {
                expected: MIN_FILE_LEN,
                found: file_len,
            });
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

        let memmap_cfg = MemMapCfg {
            module_id: MODULE_ID,
            path: cfg.path.clone(),
            version: Metadata::CURRENT_VERSION,
            initial_t: INITIAL_SLOTS,
            sync_interval: cfg.sync_interval,
            page_size: Some(PageSize::K4),
        };

        let mmap = MemMap::<Slot>::open(memmap_cfg)?;
        let mapped_len = mmap.length();
        if mapped_len != file_len {
            return Err(TurboError::InvalidSize {
                expected: file_len,
                found: mapped_len,
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
            return Err(TurboError::CorruptedHeader(
                "metadata geometric invariants violated",
            ));
        }

        let expected_bytes = (meta.total_pages as usize) * (meta.page_size as usize);
        if file_len != expected_bytes {
            return Err(TurboError::InvalidSize {
                expected: expected_bytes,
                found: file_len,
            });
        }

        if meta.root_node_vp != Metadata::DEFAULT_ROOT_NODE_VP {
            return Err(TurboError::CorruptedHeader("unexpected root node vp index"));
        }

        let is_node = mmap.read_exact::<Node, _, _>(ROOT_NODE_SLOT, |n| n.is_node())?;
        if !is_node {
            return Err(TurboError::InvalidRootNode);
        }

        let total_slots = meta.total_slots;
        Ok(Self {
            mmap,
            growth_mutex: Mutex::new(()),
            total_slots: AtomicU32::new(total_slots),
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
            let next = if i + 1 < DATABUF_SLOTS.len() {
                DATABUF_SLOTS[i + 1] as u32
            } else {
                0
            };
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
            let versioned = TurboCfg::compute_versioned_path(&path, Metadata::CURRENT_VERSION);
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
        let engine = Engine::open_or_create(cfg.clone()).unwrap();

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

        let root_node = engine
            .mmap()
            .read_exact::<Node, _, _>(ROOT_NODE_SLOT, |n| *n)
            .unwrap();
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

        let first_dbuf = engine
            .mmap()
            .read_exact::<DataBuf, _, _>(DATABUF_SLOTS[0], |d| *d)
            .unwrap();
        assert_eq!(first_dbuf.next_dbuf_index(), DATABUF_SLOTS[1] as u32);

        let last_dbuf = engine
            .mmap()
            .read_exact::<DataBuf, _, _>(DATABUF_SLOTS[0x0D], |d| *d)
            .unwrap();
        assert_eq!(last_dbuf.next_dbuf_index(), 0);

        let block1 = engine
            .mmap()
            .read_exact::<AdjArrayBlock, _, _>(1, |b| *b)
            .unwrap();
        assert_eq!(block1.next_block(), 2);

        let block5 = engine
            .mmap()
            .read_exact::<AdjArrayBlock, _, _>(5, |b| *b)
            .unwrap();
        assert_eq!(block5.next_block(), 0xC0);

        let block199 = engine
            .mmap()
            .read_exact::<AdjArrayBlock, _, _>(0xC7, |b| *b)
            .unwrap();
        assert_eq!(block199.next_block(), 0);
    }

    #[test]
    fn ok_engine_reopen_existing_valid_file() {
        let temp = TempFile::new("ok_reopen");
        let cfg = TurboCfg::new(&temp.path);

        {
            let engine = Engine::open_or_create(cfg.clone()).unwrap();
            assert_eq!(engine.total_slots(), INITIAL_SLOTS as u32);
        }

        let reopened = Engine::open_or_create(cfg).unwrap();
        assert_eq!(reopened.total_slots(), INITIAL_SLOTS as u32);
        let header = reopened.read_metadata().unwrap();
        assert_eq!(header.magic, Metadata::MAGIC);
    }

    #[test]
    fn err_engine_open_validation_size() {
        let temp = TempFile::new("err_size");
        std::fs::write(&temp.versioned, [0u8; 0x1000]).unwrap();

        let cfg = TurboCfg::new(&temp.path);
        let err = Engine::open_or_create(cfg).unwrap_err();
        match err {
            TurboError::InvalidSize { expected, found } => {
                assert_eq!(expected, MIN_FILE_LEN);
                assert_eq!(found, 0x1000);
            }
            other => panic!("unexpected error: {:?}", other),
        }
    }

    #[test]
    fn err_engine_open_validation_magic() {
        let temp = TempFile::new("err_magic");
        let cfg = TurboCfg::new(&temp.path);

        {
            let engine = Engine::open_or_create(cfg.clone()).unwrap();
            let mut bad_meta = engine.read_metadata().unwrap();
            bad_meta.magic = *b"CORR";
            engine
                .mmap()
                .write_exact::<Metadata, _, _>(0, |m| *m = bad_meta)
                .unwrap();
            engine.sync().unwrap();
        }

        let err = Engine::open_or_create(cfg).unwrap_err();
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
            let engine = Engine::open_or_create(cfg.clone()).unwrap();
            let mut bad_meta = engine.read_metadata().unwrap();
            bad_meta.version = 0x02;
            engine
                .mmap()
                .write_exact::<Metadata, _, _>(0, |m| *m = bad_meta)
                .unwrap();
            engine.sync().unwrap();
        }

        let err = Engine::open_or_create(cfg).unwrap_err();
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
            let engine = Engine::open_or_create(cfg.clone()).unwrap();
            let bucket_disguised = Bucket::new();
            engine
                .mmap()
                .write_exact::<Bucket, _, _>(ROOT_NODE_SLOT, |b| *b = bucket_disguised)
                .unwrap();
            engine.sync().unwrap();
        }

        let err = Engine::open_or_create(cfg).unwrap_err();
        match err {
            TurboError::InvalidRootNode => {}
            other => panic!("unexpected error: {:?}", other),
        }
    }
}
