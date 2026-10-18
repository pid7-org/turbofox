//! TurboFox is an embedded Key-Value database for on-device storage

mod adjarr;
mod bucket;
mod cfg;
mod databuf;
mod engine;
mod error;
mod hash;
mod list;
mod meta;
mod node;
mod slot;
mod ticket;

pub use cfg::TurboCfg;
pub use error::{TurboError, TurboResult};
pub use list::{TurboList, TurboListIter};
pub use ticket::{Completion, TEpoch, TurboTicket};

use engine::Engine;
use std::sync::Arc;

/// TurboFox is an embedded Key-Value database for on-device storage
#[derive(Debug, Clone)]
pub struct TurboFox {
    engine: Arc<Engine>,
}

impl TurboFox {
    /// Initi new instance of [`TurboFox`]
    pub fn create(cfg: TurboCfg) -> TurboResult<Self> {
        let engine = Engine::create(cfg)?;
        Ok(Self { engine: Arc::new(engine) })
    }

    /// Open an existing instance [`TurboFox`]
    pub fn open(cfg: TurboCfg) -> TurboResult<Self> {
        let engine = Engine::open(cfg)?;
        Ok(Self { engine: Arc::new(engine) })
    }

    /// Insert new or update existing Key-Value pair
    #[inline(always)]
    pub fn set(&self, key: &[u8], val: &[u8]) -> TurboResult<TurboTicket> {
        self.engine.set(key, val)
    }

    /// Retrieve value payload for the specified key
    #[inline(always)]
    pub fn get(&self, key: &[u8]) -> TurboResult<Option<Vec<u8>>> {
        self.engine.get(key)
    }

    /// Delete an existing Key-Value or [`TurboList`] entry from [`TurboFox`]
    ///
    /// It returns `Ok(true)` if entry existed and was deleted, `Ok(false)` if entry was not found
    #[inline(always)]
    pub fn del(&self, key: &[u8]) -> TurboResult<bool> {
        self.engine.del(key)
    }

    /// Access or create a new [`TurboList`] instance
    #[inline(always)]
    pub fn list(&self, key: &[u8]) -> TurboList {
        TurboList::new(Arc::clone(&self.engine), key.to_vec())
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
