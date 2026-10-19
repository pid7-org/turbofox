//! Configuration definitions for TurboFox engine instance
//!
//! ## Example
//!
//! ```
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

pub(crate) const MODULE_ID: u8 = 0x46;
pub(crate) const PAGE_SIZE: u32 = 0x1000;
pub(crate) const DEFAULT_VERSION: u32 = 1;
pub(crate) const MAX_BUF_IDX: u32 = 0x7FFF_FFFF;
pub(crate) const MAX_DBUF_SLOT: u32 = MAX_BUF_IDX / 0x3F;
pub(crate) const MAX_STORAGE_PAGES: u32 = MAX_DBUF_SLOT / 0x20;

/// Configuration parameters for initializing or opening a [`crate::TurboFox`] instance
///
/// ## Example
///
/// ```
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
    /// ```
    /// use turbofox::TurboCfg;
    ///
    /// let cfg = TurboCfg::new("default.db");
    /// assert_eq!(cfg.version, 1);
    /// assert!(cfg.sync_interval.is_none());
    /// ```
    #[inline]
    pub fn new<P: Into<PathBuf>>(path: P) -> Self {
        Self { path: path.into(), version: DEFAULT_VERSION, sync_interval: None }
    }

    /// Configures layout and backing file version to enable schema migration or multi-version file
    /// co-existence
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::TurboCfg;
    ///
    /// let cfg = TurboCfg::new("versioned.db").with_version(2);
    /// assert_eq!(cfg.version, 2);
    /// ```
    #[inline]
    pub fn with_version(mut self, version: u32) -> Self {
        self.version = version;
        self
    }

    /// Configures periodic background flush interval
    ///
    /// ## Example
    ///
    /// ```
    /// use std::time::Duration;
    /// use turbofox::TurboCfg;
    ///
    /// let cfg = TurboCfg::new("periodic.db")
    ///     .with_sync_interval(Duration::from_millis(0x64));
    /// assert_eq!(cfg.sync_interval, Some(Duration::from_millis(0x64)));
    /// ```
    #[inline]
    pub fn with_sync_interval(mut self, interval: Duration) -> Self {
        self.sync_interval = Some(interval);
        self
    }
}
