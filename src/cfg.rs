//! Configuration definitions for TurboFox engine instance

use std::path::PathBuf;
use std::time::Duration;

/// Engine module identifier for error context bitpacking
pub(crate) const MODULE_ID: u8 = 0x46;

/// Operating system physical page size constraint in bytes (4 KiB)
pub(crate) const PAGE_SIZE: u32 = 0x1000;

/// Default database file format version
pub(crate) const DEFAULT_VERSION: u32 = 1;

/// Configuration options for initializing a [`crate::TurboFox`] instance
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
    /// Creates a configuration with default parameters for given target path
    #[inline]
    pub fn new<P: Into<PathBuf>>(path: P) -> Self {
        Self { path: path.into(), version: DEFAULT_VERSION, sync_interval: None }
    }

    /// Configures layout and backing file version
    #[inline]
    pub fn with_version(mut self, version: u32) -> Self {
        self.version = version;
        self
    }

    /// Configures periodic background flush interval
    #[inline]
    pub fn with_sync_interval(mut self, interval: Duration) -> Self {
        self.sync_interval = Some(interval);
        self
    }
}
