//! TurboFox embedded key-value storage engine

use std::sync::Arc;

pub mod cfg;
pub mod error;

pub(crate) mod adjarr;
pub(crate) mod bucket;
pub(crate) mod databuf;
pub(crate) mod engine;
pub mod hash;
pub(crate) mod meta;
pub(crate) mod node;
pub(crate) mod slot;

pub use cfg::{MODULE_ID, PAGE_SIZE, TurboCfg};
pub use error::{TurboError, TurboResult};
pub use frozen_core::ack::AckTicket;

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
        Ok(Self { engine: Arc::new(engine) })
    }

    /// Sets a key-value pair, expanding storage dynamically when needed
    #[inline]
    pub fn set(&self, key: &[u8], val: &[u8]) -> TurboResult<AckTicket> {
        self.engine.set(key, val)
    }

    /// Retrieves value payload for the specified key
    #[inline]
    pub fn get(&self, key: &[u8]) -> TurboResult<Option<Vec<u8>>> {
        self.engine.get(key)
    }

    /// Total active key-value and list entries in the database
    #[inline]
    pub fn total_entries(&self) -> u64 {
        self.engine.total_entries()
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

    /// Unmaps memory, terminates background sync, and removes database from filesystem
    pub fn delete(self) -> TurboResult<()> {
        match Arc::try_unwrap(self.engine) {
            Ok(engine) => engine.delete(),
            Err(_) => Err(TurboError::ResourceBusy(
                "cannot delete database with active concurrent handles",
            )),
        }
    }
}
