//! Configuration parameters for initializing or opening a [`crate::TurboFox`] instance
//!
//! ## Example
//!
//! ```
//! use std::path::PathBuf;
//! use std::time::Duration;
//! use turbofox::{SyncMode, TurboCfg};
//!
//! let cfg = TurboCfg::periodic("turbofox_data", Duration::from_millis(0x32));
//!
//! assert_eq!(cfg.path, PathBuf::from("turbofox_data"));
//! assert_eq!(cfg.sync_mode, SyncMode::Periodic(Duration::from_millis(0x32)));
//! ```

use std::path::PathBuf;
use std::time::Duration;

#[allow(unused_imports)]
pub(crate) use crate::common::{
    DEFAULT_VERSION, MAX_BUF_IDX, MAX_DBUF_SLOT, MAX_STORAGE_PAGES, MODULE_ID, PAGE_SIZE,
};

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

    /// Background thread disabled; caller explicitly drives durability via [`crate::TurboFox::sync`]
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
        Self {
            path: path.into(),
            sync_mode,
        }
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

    /// Configure periodic background flush interval for committing dirty pages
    ///
    /// ## Example
    ///
    /// ```
    /// use std::time::Duration;
    /// use turbofox::{SyncMode, TurboCfg};
    ///
    /// let cfg = TurboCfg::manual("periodic_data")
    ///     .with_sync_interval(Duration::from_millis(0x64));
    /// assert_eq!(cfg.sync_mode, SyncMode::Periodic(Duration::from_millis(0x64)));
    /// ```
    #[inline]
    pub fn with_sync_interval(mut self, interval: Duration) -> Self {
        self.sync_mode = SyncMode::Periodic(interval);
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
        assert_eq!(manual.resolve_dir(), PathBuf::from("test_db_dir"));

        let periodic = TurboCfg::periodic("test_db_dir", Duration::from_millis(0x64));
        assert_eq!(periodic.sync_mode, SyncMode::Periodic(Duration::from_millis(0x64)));
        assert_eq!(periodic.sync_interval(), Some(Duration::from_millis(0x64)));

        let updated = manual.with_sync_mode(SyncMode::Periodic(Duration::from_millis(0x32)));
        assert_eq!(updated.sync_mode, SyncMode::Periodic(Duration::from_millis(0x32)));

        let with_interval = TurboCfg::manual("test_db_dir")
            .with_sync_interval(Duration::from_millis(0x10));
        assert_eq!(with_interval.sync_mode, SyncMode::Periodic(Duration::from_millis(0x10)));
    }
}
