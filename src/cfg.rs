//! Configuration parameters for initializing or opening a [`crate::TurboFox`] instance
//!
//! ## Example
//!
//! ```
//! use std::{path::PathBuf, time::Duration};
//! use turbofox::{CapacityHint, SyncMode, TurboCfg};
//!
//! let hint = CapacityHint::new().with_kv(0x0001_86A0, 0x10, 0x20);
//! let cfg = TurboCfg::periodic("turbofox_data", Duration::from_millis(0x32))
//!     .with_capacity_hint(hint);
//!
//! assert_eq!(cfg.path, PathBuf::from("turbofox_data"));
//! assert_eq!(cfg.sync_mode, SyncMode::Periodic(Duration::from_millis(0x32)));
//! assert_eq!(cfg.capacity_hint, Some(hint));
//! ```

use crate::{
    common::{
        ADJ_ARR_SLOTS, BUCKET_SLOTS, DBUF_BUF_COUNT, DBUF_BUF_SIZE, DBUF_SLOTS, KV_HEADER_SIZE,
        LIST_KEY_HEADER_SIZE, LIST_VAL_HEADER_SIZE, MAX_DB_LEN, SLOT_SIZE,
    },
    error::{TurboError, TurboResult},
};
use frozen_core::memmap::PageSize;
use std::{path::PathBuf, time::Duration};

/// Capacity hint guiding upfront pre-allocation of the backing database file
///
/// Pre-sizes backing storage to accommodate expected key-value or list workloads, eliminating
/// runtime `mmap` expansion overhead and p99+ latency spikes during high-throughput ingestion;
///
/// Calculated sizing aligns to the host operating system growth quantum (16 KiB on 4 KiB and
/// 16 KiB page systems; 64 KiB on 64 KiB page systems)
///
/// ## Example
///
/// ```
/// use turbofox::CapacityHint;
///
/// let hint = CapacityHint::new()
///     .with_kv(0x0004_0000, 0x10, 0x20)
///     .with_list(0x0400, 0x08, 0x40);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CapacityHint {
    expected_kv_count: usize,
    avg_key_len: usize,
    avg_val_len: usize,
    expected_list_count: usize,
    avg_list_items: usize,
    avg_item_len: usize,
}

impl CapacityHint {
    /// Construct an empty capacity hint with all expectations initialized to zero
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::CapacityHint;
    ///
    /// let hint = CapacityHint::new();
    /// ```
    #[inline(always)]
    pub const fn new() -> Self {
        Self {
            expected_kv_count: 0x00,
            avg_key_len: 0x00,
            avg_val_len: 0x00,
            expected_list_count: 0x00,
            avg_list_items: 0x00,
            avg_item_len: 0x00,
        }
    }

    /// Configure expected key-value entries alongside average key and value byte lengths
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::CapacityHint;
    ///
    /// let hint = CapacityHint::new().with_kv(0x0001_86A0, 0x10, 0x20);
    /// ```
    #[inline(always)]
    pub const fn with_kv(
        mut self,
        expected_count: usize,
        avg_key_len: usize,
        avg_val_len: usize,
    ) -> Self {
        self.expected_kv_count = expected_count;
        self.avg_key_len = avg_key_len;
        self.avg_val_len = avg_val_len;

        self
    }

    /// Configure expected list entries alongside average item counts and payload lengths
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::CapacityHint;
    ///
    /// let hint = CapacityHint::new().with_list(0x1388, 0x0A, 0x40);
    /// ```
    #[inline(always)]
    pub const fn with_list(
        mut self,
        expected_count: usize,
        avg_list_items: usize,
        avg_item_len: usize,
    ) -> Self {
        self.expected_list_count = expected_count;
        self.avg_list_items = avg_list_items;
        self.avg_item_len = avg_item_len;

        self
    }

    pub(crate) fn compute_initial_pages(&self, page_size: PageSize) -> TurboResult<usize> {
        let page_bytes = page_size.as_usize();

        // Base topography: 128 slots = 8192 bytes
        let mut total_slots = 0x80usize;
        if self.expected_kv_count > 0x00 {
            let bufs_per_kv =
                (KV_HEADER_SIZE + self.avg_key_len + self.avg_val_len).div_ceil(DBUF_BUF_SIZE);

            let total_bufs = self.expected_kv_count.saturating_mul(bufs_per_kv);
            let dbufs = total_bufs.div_ceil(DBUF_BUF_COUNT);
            total_slots = total_slots.saturating_add(dbufs.saturating_mul(DBUF_SLOTS));

            // HAMT buckets & internal nodes (~65% load factor: 13 entries per bucket of 20)
            let buckets = self.expected_kv_count.div_ceil(0x0D);
            total_slots = total_slots.saturating_add(buckets.saturating_mul(BUCKET_SLOTS));
        }

        if self.expected_list_count > 0x00 {
            let key_bufs = (LIST_KEY_HEADER_SIZE + self.avg_key_len).div_ceil(DBUF_BUF_SIZE);
            let item_bufs = (LIST_VAL_HEADER_SIZE + self.avg_item_len).div_ceil(DBUF_BUF_SIZE);

            let total_item_bufs = self
                .expected_list_count
                .saturating_mul(self.avg_list_items)
                .saturating_mul(item_bufs);
            let total_list_bufs =
                self.expected_list_count.saturating_mul(key_bufs).saturating_add(total_item_bufs);

            let dbufs = total_list_bufs.div_ceil(DBUF_BUF_COUNT);
            total_slots = total_slots.saturating_add(dbufs.saturating_mul(DBUF_SLOTS));

            // AdjArray blocks: each block holds 6 list heads (24 item entries)
            let adj_items = self.expected_list_count.saturating_mul(self.avg_list_items.max(0x01));
            let adj_blocks = adj_items.div_ceil(0x18);

            total_slots = total_slots.saturating_add(adj_blocks.saturating_mul(ADJ_ARR_SLOTS));
        }

        let raw_bytes = total_slots.saturating_mul(SLOT_SIZE);
        if raw_bytes > MAX_DB_LEN {
            return Err(TurboError::DatabaseFull);
        }

        let quantum_pages = match page_size {
            PageSize::K4 => 0x04,
            PageSize::K16 | PageSize::K64 => 0x01,
        };
        let quantum_bytes = quantum_pages * page_bytes;

        let aligned_bytes = raw_bytes.max(quantum_bytes).div_ceil(quantum_bytes) * quantum_bytes;
        if aligned_bytes > MAX_DB_LEN {
            return Err(TurboError::DatabaseFull);
        }

        let pages = aligned_bytes / page_bytes;
        Ok(pages.max(quantum_pages))
    }
}

/// Durability synchronization mode driving engine persistence
///
/// Dictates whether dirty pages are periodically flushed to physical storage by a background
/// worker thread, or if persistence requires manual coordination by the caller
///
/// ## Example
///
/// ```
/// use std::time::Duration;
/// use turbofox::{SyncMode, TurboCfg};
///
/// let periodic_cfg = TurboCfg::periodic("data_dir", Duration::from_millis(0x64));
/// assert_eq!(periodic_cfg.sync_mode, SyncMode::Periodic(Duration::from_millis(0x64)));
///
/// let manual_cfg = TurboCfg::manual("data_dir");
/// assert_eq!(manual_cfg.sync_mode, SyncMode::Manual);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    /// Background worker coalesces and flushes dirty pages at the configured interval
    Periodic(Duration),

    /// Background thread disabled; caller explicitly flushes durability
    /// via [`crate::TurboFox::sync`]
    Manual,
}

/// Configuration parameters for initializing or opening a [`crate::TurboFox`] instance
///
/// Path must point strictly to a target directory and not a file; engine backing files are
/// managed internally within this directory
///
/// ## Example
///
/// ```
/// use std::path::PathBuf;
/// use turbofox::{SyncMode, TurboCfg};
///
/// let cfg = TurboCfg::manual(PathBuf::from("data_dir"));
/// assert_eq!(cfg.sync_mode, SyncMode::Manual);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurboCfg {
    /// Filesystem directory hosting memory-mapped database files; must be a directory
    pub path: PathBuf,

    /// Durability synchronization mode driving background flush worker or manual commits
    pub sync_mode: SyncMode,

    /// Optional capacity hint guiding upfront backing storage pre-allocation
    pub capacity_hint: Option<CapacityHint>,

    /// Optional in-memory lookup cache to accelerate record access; defaults to false
    pub lookup_cache: bool,
}

impl TurboCfg {
    /// Construct configuration with an explicit synchronization mode
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{SyncMode, TurboCfg};
    ///
    /// let cfg = TurboCfg::new("data_dir", SyncMode::Manual);
    /// assert_eq!(cfg.sync_mode, SyncMode::Manual);
    /// ```
    #[inline]
    pub fn new<P: Into<PathBuf>>(path: P, sync_mode: SyncMode) -> Self {
        Self { path: path.into(), sync_mode, capacity_hint: None, lookup_cache: false }
    }

    /// Construct configuration driving a periodic background synchronization thread
    ///
    /// When an interval is set, the engine spawns a dedicated background synchronization thread
    /// that coalesces and commits dirty memory-mapped pages to storage
    ///
    /// While the database is idle (`target_epoch == durable_epoch`), the worker thread parks on an
    /// internal condition variable, consuming zero CPU cycles; incoming write operations wake the
    /// worker, which delays for `interval` to batch concurrent writes before issuing `msync`
    ///
    /// As flushes finish, the worker increments the global durable epoch counter and signals any
    /// callers awaiting [`TurboTicket`](crate::TurboTicket) handles
    ///
    /// ## Example
    ///
    /// ```
    /// use std::time::Duration;
    /// use turbofox::{SyncMode, TurboCfg};
    ///
    /// let cfg = TurboCfg::periodic("data_dir", Duration::from_millis(0x64));
    /// assert_eq!(cfg.sync_mode, SyncMode::Periodic(Duration::from_millis(0x64)));
    /// ```
    #[inline]
    pub fn periodic<P: Into<PathBuf>>(path: P, interval: Duration) -> Self {
        Self {
            path: path.into(),
            sync_mode: SyncMode::Periodic(interval),
            capacity_hint: None,
            lookup_cache: false,
        }
    }

    /// Construct configuration with manual synchronization and no background worker thread
    ///
    /// The engine runs without spawning a background flush thread; write operations commit to
    /// memory-mapped buffers immediately and persist to physical storage only when the caller
    /// invokes [`crate::TurboFox::sync`] or [`crate::TurboTicket::force`], or when evicted by the
    /// operating system page cache
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{SyncMode, TurboCfg};
    ///
    /// let cfg = TurboCfg::manual("data_dir");
    /// assert_eq!(cfg.sync_mode, SyncMode::Manual);
    /// ```
    #[inline]
    pub fn manual<P: Into<PathBuf>>(path: P) -> Self {
        Self {
            path: path.into(),
            sync_mode: SyncMode::Manual,
            capacity_hint: None,
            lookup_cache: false,
        }
    }

    /// Update the synchronization mode
    ///
    /// ## Example
    ///
    /// ```
    /// use std::time::Duration;
    /// use turbofox::{SyncMode, TurboCfg};
    ///
    /// let cfg = TurboCfg::manual("data_dir")
    ///     .with_sync_mode(SyncMode::Periodic(Duration::from_millis(0x64)));
    /// assert_eq!(cfg.sync_mode, SyncMode::Periodic(Duration::from_millis(0x64)));
    /// ```
    #[inline]
    pub fn with_sync_mode(mut self, sync_mode: SyncMode) -> Self {
        self.sync_mode = sync_mode;
        self
    }

    /// Attach a capacity hint guiding upfront backing file pre-allocation
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{CapacityHint, TurboCfg};
    ///
    /// let hint = CapacityHint::new().with_kv(0x0001_86A0, 0x10, 0x20);
    /// let cfg = TurboCfg::manual("data_dir").with_capacity_hint(hint);
    /// assert_eq!(cfg.capacity_hint, Some(hint));
    /// ```
    #[inline]
    pub fn with_capacity_hint(mut self, hint: CapacityHint) -> Self {
        self.capacity_hint = Some(hint);
        self
    }

    /// Configure optional in-memory lookup cache enablement
    ///
    /// When enabled, the engine maintains an append-only in-memory resolution cache providing
    /// wait-free O(1) record addressing across grown storage segments at the cost of modest
    /// resident memory; when disabled (default), record locations are resolved by traversing
    /// persistent storage structures directly
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::TurboCfg;
    ///
    /// let cfg = TurboCfg::manual("data_dir").with_lookup_cache(true);
    /// assert!(cfg.lookup_cache);
    /// ```
    #[inline]
    pub fn with_lookup_cache(mut self, enabled: bool) -> Self {
        self.lookup_cache = enabled;
        self
    }

    /// Return optional synchronization duration for underlying memory-mapping configuration
    #[inline(always)]
    pub(crate) fn sync_interval(&self) -> Option<Duration> {
        match self.sync_mode {
            SyncMode::Periodic(interval) => Some(interval),
            SyncMode::Manual => None,
        }
    }

    #[inline]
    pub(crate) fn resolve_dir(&self) -> PathBuf {
        self.path.clone()
    }

    #[inline(always)]
    pub(crate) fn resolve_initial_pages(&self, page_size: PageSize) -> TurboResult<usize> {
        match self.capacity_hint {
            Some(hint) => hint.compute_initial_pages(page_size),
            None => Ok(match page_size {
                PageSize::K4 => 0x04,
                PageSize::K16 | PageSize::K64 => 0x01,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_turbocfg_sync_modes() {
        let manual = TurboCfg::manual("test_db_dir");
        assert_eq!(manual.path, PathBuf::from("test_db_dir"));
        assert_eq!(manual.sync_mode, SyncMode::Manual);
        assert_eq!(manual.sync_interval(), None);
        assert_eq!(manual.capacity_hint, None);
        assert!(!manual.lookup_cache);
        assert_eq!(manual.resolve_dir(), PathBuf::from("test_db_dir"));

        let periodic = TurboCfg::periodic("test_db_dir", Duration::from_millis(0x64));
        assert_eq!(periodic.sync_mode, SyncMode::Periodic(Duration::from_millis(0x64)));
        assert_eq!(periodic.sync_interval(), Some(Duration::from_millis(0x64)));
        assert_eq!(periodic.capacity_hint, None);
        assert!(!periodic.lookup_cache);

        let updated = manual.with_sync_mode(SyncMode::Periodic(Duration::from_millis(0x32)));
        assert_eq!(updated.sync_mode, SyncMode::Periodic(Duration::from_millis(0x32)));
    }

    #[test]
    fn ok_turbocfg_lookup_cache_builder() {
        let default_cfg = TurboCfg::manual("test_db_dir");
        assert!(!default_cfg.lookup_cache);

        let enabled_cfg = default_cfg.with_lookup_cache(true);
        assert!(enabled_cfg.lookup_cache);

        let disabled_cfg = enabled_cfg.with_lookup_cache(false);
        assert!(!disabled_cfg.lookup_cache);
    }

    #[test]
    fn ok_capacity_hint_builder() {
        let hint =
            CapacityHint::new().with_kv(0x0001_86A0, 0x10, 0x20).with_list(0x03E8, 0x05, 0x20);

        let cfg = TurboCfg::manual("test_db_dir").with_capacity_hint(hint);
        assert_eq!(cfg.capacity_hint, Some(hint));
    }

    #[test]
    fn ok_capacity_hint_quantum_alignment() {
        let hint = CapacityHint::new().with_kv(0x03E8, 0x10, 0x20);

        let pages_4k = hint.compute_initial_pages(PageSize::K4).unwrap();
        let bytes_4k = pages_4k * 0x1000;
        assert_eq!(bytes_4k % 0x4000, 0x00);
        assert!(bytes_4k >= 0x4000);

        let pages_16k = hint.compute_initial_pages(PageSize::K16).unwrap();
        let bytes_16k = pages_16k * 0x4000;
        assert_eq!(bytes_16k % 0x4000, 0x00);
        assert!(bytes_16k >= 0x4000);

        let pages_64k = hint.compute_initial_pages(PageSize::K64).unwrap();
        let bytes_64k = pages_64k * 0x0001_0000;
        assert_eq!(bytes_64k % 0x0001_0000, 0x00);
        assert!(bytes_64k >= 0x0001_0000);
    }

    #[test]
    fn ok_empty_capacity_hint_defaults() {
        let empty = CapacityHint::new();
        assert_eq!(empty.compute_initial_pages(PageSize::K4).unwrap(), 0x04);
        assert_eq!(empty.compute_initial_pages(PageSize::K16).unwrap(), 0x01);
        assert_eq!(empty.compute_initial_pages(PageSize::K64).unwrap(), 0x01);

        let cfg = TurboCfg::manual("test_db_dir");
        assert_eq!(cfg.resolve_initial_pages(PageSize::K4).unwrap(), 0x04);
        assert_eq!(cfg.resolve_initial_pages(PageSize::K16).unwrap(), 0x01);
        assert_eq!(cfg.resolve_initial_pages(PageSize::K64).unwrap(), 0x01);
    }

    #[test]
    fn err_capacity_hint_overflow() {
        let huge_hint = CapacityHint::new().with_kv(usize::MAX / 0x02, 0x0400, 0x0400);
        assert!(matches!(
            huge_hint.compute_initial_pages(PageSize::K4),
            Err(TurboError::DatabaseFull)
        ));
    }
}
