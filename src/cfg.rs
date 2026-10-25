//! Configuration definitions for TurboFox engine instance
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
/// Path must point strictly to an existing or target directory and not a file;
/// engine backing files are managed internally within this directory
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
    /// Filesystem directory path hosting database storage files; must be a directory and not a file
    pub path: PathBuf,

    /// Optional interval for background synchronization worker
    pub sync_interval: Option<Duration>,
}

impl TurboCfg {
    /// Creates a configuration with default parameters for the specified target directory;
    /// path must strictly be a directory and not a file
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

    /// Configures periodic background flush interval
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

    /// Ref: `DESIGN.md` §8.2
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
