//! Configuration definitions for TurboFox engine instance
//!
//! ## Example
//!
//! ```no_run
//! use std::path::PathBuf;
//! use std::time::Duration;
//! use turbofox::TurboCfg;
//!
//! let cfg = TurboCfg::new("storage.db")
//!     .with_version(1)
//!     .with_sync_interval(Duration::from_millis(0x32));
//!
//! assert_eq!(cfg.path, PathBuf::from("storage.db"));
//! assert_eq!(cfg.version, 1);
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
/// ## Example
///
/// ```no_run
/// use std::path::PathBuf;
/// use turbofox::TurboCfg;
///
/// let cfg = TurboCfg::new(PathBuf::from("node.db"));
/// assert_eq!(cfg.version, 1);
/// assert!(cfg.sync_interval.is_none());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurboCfg {
    /// Filesystem path to database storage file
    pub path: PathBuf,

    /// Format and backing file version
    pub version: u32,

    /// Optional interval for background synchronization worker
    pub sync_interval: Option<Duration>,
}

impl TurboCfg {
    /// Creates a configuration with default parameters for the specified target path
    ///
    /// ## Example
    ///
    /// ```no_run
    /// use turbofox::TurboCfg;
    ///
    /// let cfg = TurboCfg::new("default.db");
    /// assert_eq!(cfg.version, 1);
    /// assert!(cfg.sync_interval.is_none());
    /// ```
    #[inline]
    pub fn new<P: Into<PathBuf>>(_path: P) -> Self {
        todo!()
    }

    /// Configures layout and backing file version to enable schema migration or multi-version file
    /// co-existence
    ///
    /// ## Example
    ///
    /// ```no_run
    /// use turbofox::TurboCfg;
    ///
    /// let cfg = TurboCfg::new("versioned.db").with_version(2);
    /// assert_eq!(cfg.version, 2);
    /// ```
    #[inline]
    pub fn with_version(self, _version: u32) -> Self {
        todo!()
    }

    /// Configures periodic background flush interval
    ///
    /// ## Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    /// use turbofox::TurboCfg;
    ///
    /// let cfg = TurboCfg::new("periodic.db")
    ///     .with_sync_interval(Duration::from_millis(0x64));
    /// assert_eq!(cfg.sync_interval, Some(Duration::from_millis(0x64)));
    /// ```
    #[inline]
    pub fn with_sync_interval(self, _interval: Duration) -> Self {
        todo!()
    }
}
