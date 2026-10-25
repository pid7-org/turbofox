//! Configuration parameters for initializing or opening a [`crate::TurboFox`] instance
//!
//! ## Example
//!
//! ```
//! use std::path::PathBuf;
//! use std::time::Duration;
//! use turbofox::TurboCfg;
//!
//! let cfg = TurboCfg::new("turbofox_data")
//!     .with_sync_interval(Duration::from_millis(0x32));
//!
//! assert_eq!(cfg.path, PathBuf::from("turbofox_data"));
//! assert_eq!(cfg.sync_interval, Some(Duration::from_millis(0x32)));
//! ```

use std::path::PathBuf;
use std::time::Duration;

#[allow(unused_imports)]
pub(crate) use crate::common::{
    DEFAULT_VERSION, MAX_BUF_IDX, MAX_DBUF_SLOT, MAX_STORAGE_PAGES, MODULE_ID, PAGE_SIZE,
};

/// Configuration parameters for initializing or opening a [`crate::TurboFox`] instance
///
/// Path must point strictly to a target directory and not a file; engine backing files are
/// managed internally within this directory
///
/// ## Example
///
/// ```
/// use std::path::PathBuf;
/// use turbofox::TurboCfg;
///
/// let cfg = TurboCfg::new(PathBuf::from("data_dir"));
/// assert!(cfg.sync_interval.is_none());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurboCfg {
    /// Filesystem directory hosting memory-mapped database files; must be a directory
    pub path: PathBuf,

    /// Optional flush interval driving the background persistence worker thread
    pub sync_interval: Option<Duration>,
}

impl TurboCfg {
    /// Construct configuration for the specified directory with default sync disabled
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::TurboCfg;
    ///
    /// let cfg = TurboCfg::new("data_dir");
    /// assert!(cfg.sync_interval.is_none());
    /// ```
    #[inline]
    pub fn new<P: Into<PathBuf>>(path: P) -> Self {
        Self { path: path.into(), sync_interval: None }
    }

    /// Configure periodic background flush interval for committing dirty pages
    ///
    /// ## Background Worker Architecture
    ///
    /// When an interval is set, the engine spawns a dedicated background synchronization thread
    /// that coalesces and commits dirty memory-mapped pages to storage
    ///
    /// While the database is idle (`target_epoch == durable_epoch`), the worker thread parks on an
    /// internal condition variable, consuming zero CPU cycles; incoming write operations wake the
    /// worker, which delays for `interval` to batch concurrent writes before issuing `msync`
    ///
    /// As flushes finish, the worker increments the global durable epoch counter and signals any
    /// callers awaiting [`TurboTicket`](crate::TurboTicket) handles; without an interval, pages
    /// persist only via explicit [`crate::TurboFox::sync`] calls or OS page cache writeback
    ///
    /// ## Example
    ///
    /// ```
    /// use std::time::Duration;
    /// use turbofox::TurboCfg;
    ///
    /// let cfg = TurboCfg::new("periodic_data")
    ///     .with_sync_interval(Duration::from_millis(0x64));
    /// assert_eq!(cfg.sync_interval, Some(Duration::from_millis(0x64)));
    /// ```
    #[inline]
    pub fn with_sync_interval(mut self, interval: Duration) -> Self {
        self.sync_interval = Some(interval);
        self
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
    fn ok_turbocfg_defaults_and_builder() {
        let cfg = TurboCfg::new("test_db_dir");
        assert_eq!(cfg.path, PathBuf::from("test_db_dir"));
        assert_eq!(cfg.sync_interval, None);
        assert_eq!(cfg.resolve_dir(), PathBuf::from("test_db_dir"));

        let with_sync = cfg.with_sync_interval(Duration::from_millis(0x64));
        assert_eq!(with_sync.sync_interval, Some(Duration::from_millis(0x64)));
    }
}
