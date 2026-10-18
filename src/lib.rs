//! NA

mod adjarr;
mod bucket;
mod cfg;
mod databuf;
mod engine;
mod error;
mod hash;
mod meta;
mod node;
mod slot;
mod ticket;

pub use cfg::TurboCfg;
pub use error::{TurboError, TurboResult};
pub use frozen_core::ack::AckTicket;
pub use ticket::{Completion, TEpoch, TurboTicket};

use engine::Engine;
use std::sync::Arc;

/// Embedded Key-Value database handle
#[derive(Debug, Clone)]
pub struct TurboFox {
    engine: Arc<Engine>,
}

impl TurboFox {
    /// Initializes brand new database backing file and layout;
    /// fails if backing file already exists
    pub fn create(cfg: TurboCfg) -> TurboResult<Self> {
        let engine = Engine::create(cfg)?;
        Ok(Self { engine: Arc::new(engine) })
    }

    /// Opens existing database file and verifies structure invariants;
    /// fails if backing file does not exist
    pub fn open(cfg: TurboCfg) -> TurboResult<Self> {
        let engine = Engine::open(cfg)?;
        Ok(Self { engine: Arc::new(engine) })
    }

    /// Sets a key-value pair, expanding storage dynamically when needed
    #[inline(always)]
    pub fn set(&self, key: &[u8], val: &[u8]) -> TurboResult<TurboTicket> {
        self.engine.set(key, val)
    }

    /// Retrieves value payload for the specified key
    #[inline(always)]
    pub fn get(&self, key: &[u8]) -> TurboResult<Option<Vec<u8>>> {
        self.engine.get(key)
    }

    /// Deletes a key-value entry from database;
    /// returns Ok(true) if entry existed and was deleted, Ok(false) if entry was not found
    #[inline(always)]
    pub fn del(&self, key: &[u8]) -> TurboResult<bool> {
        self.engine.del(key)
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
