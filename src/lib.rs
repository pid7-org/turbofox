//! TurboFox embedded key-value storage engine

use std::sync::Arc;

pub(crate) mod adjarr;
pub(crate) mod bucket;
pub mod cfg;
pub(crate) mod databuf;
pub(crate) mod engine;
pub mod error;
pub(crate) mod hash;
pub(crate) mod meta;
pub(crate) mod node;
pub(crate) mod slot;

pub use cfg::{MODULE_ID, PAGE_SIZE, TurboCfg};
pub use error::{TurboError, TurboResult};

use engine::Engine;

/// Embedded Key-Value database handle
#[derive(Debug, Clone)]
pub struct TurboFox {
    pub(crate) engine: Arc<Engine>,
}

impl TurboFox {
    /// Opens an existing database or formats a new one per provided configuration
    pub fn new(cfg: TurboCfg) -> TurboResult<Self> {
        let engine = Engine::open_or_create(cfg)?;
        Ok(Self {
            engine: Arc::new(engine),
        })
    }

    /// Flushes dirty mapped pages to durable storage writeback
    #[inline]
    pub fn sync(&self) -> TurboResult<()> {
        self.engine.sync()
    }

    /// Total operational slots allocated in the mapping
    #[inline]
    pub fn total_slots(&self) -> u32 {
        self.engine.total_slots()
    }

    /// Consumes database handle, unmaps memory, and deletes backing file from storage
    pub fn delete(self) -> TurboResult<()> {
        match Arc::try_unwrap(self.engine) {
            Ok(engine) => engine.delete(),
            Err(_) => Err(TurboError::ResourceBusy(
                "cannot delete database with active concurrent handles",
            )),
        }
    }
}
