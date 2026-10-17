//! Configuration definitions for TurboFox engine instance

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::meta::Metadata;

/// Engine module identifier for error context bitpacking
pub const MODULE_ID: u8 = 0x46;

/// Operating system physical page size constraint in bytes (4 KiB)
pub const PAGE_SIZE: u32 = 0x1000;

/// Configuration options for initializing a [`crate::TurboFox`] instance
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurboCfg {
    /// Filesystem path to database storage file
    pub path: PathBuf,
    /// Optional interval for background synchronization worker
    pub sync_interval: Option<Duration>,
}

impl TurboCfg {
    /// Creates a configuration with default parameters for given target path
    #[inline]
    pub fn new<P: Into<PathBuf>>(path: P) -> Self {
        Self {
            path: path.into(),
            sync_interval: None,
        }
    }

    /// Configures periodic background flush interval
    #[inline]
    pub fn with_sync_interval(mut self, interval: Duration) -> Self {
        self.sync_interval = Some(interval);
        self
    }

    /// Computes layout-versioned filename path matching underlying memory map conventions
    #[inline]
    pub fn versioned_path(&self) -> PathBuf {
        Self::compute_versioned_path(&self.path, Metadata::CURRENT_VERSION)
    }

    /// Derives versioned path for specified path and version index
    pub fn compute_versioned_path(base: &Path, version: u32) -> PathBuf {
        let parent = base.parent();
        let stem = base.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let extension = base.extension().and_then(|e| e.to_str());

        let filename = match extension {
            Some(ext) if !ext.is_empty() => format!("{}_{}.{}", stem, version, ext),
            _ => format!("{}_{}", stem, version),
        };

        match parent {
            Some(p) if !p.as_os_str().is_empty() => p.join(filename),
            _ => PathBuf::from(filename),
        }
    }
}
