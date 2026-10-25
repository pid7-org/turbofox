//! TurboFox is an embedded key-value storage engine engineered for on-device persistence
//!
//! ## Example (Key-Value)
//!
//! ```
//! use turbofox::{TurboCfg, TurboFox};
//!
//! let dir = tempfile::tempdir().unwrap();
//! let path = dir.path();
//! let cfg = TurboCfg::new(path);
//! let db = TurboFox::create(cfg).unwrap();
//!
//! let ticket = db.set(b"device:01", b"state:active").unwrap();
//! assert_eq!(db.get(b"device:01").unwrap(), Some(b"state:active".to_vec()));
//!
//! db.sync().unwrap();
//! assert!(ticket.is_durable());
//!
//! assert!(db.del(b"device:01").unwrap());
//! assert_eq!(db.get(b"device:01").unwrap(), None);
//!
//! db.delete().unwrap();
//! ```
//!
//! ## Example (`TurboList`)
//!
//! ```
//! use turbofox::{TurboCfg, TurboFox};
//!
//! let dir = tempfile::tempdir().unwrap();
//! let path = dir.path();
//! let cfg = TurboCfg::new(path);
//! let db = TurboFox::create(cfg).unwrap();
//!
//! let list = db.list(b"events");
//! list.append(b"boot").unwrap();
//! list.append(b"init").unwrap();
//!
//! assert_eq!(list.len().unwrap(), 2);
//! assert_eq!(list.pop().unwrap(), Some(b"init".to_vec()));
//! assert_eq!(list.pop().unwrap(), Some(b"boot".to_vec()));
//! assert_eq!(list.pop().unwrap(), None);
//!
//! drop(list);
//! db.delete().unwrap();
//! ```

use std::sync::Arc;

mod adjarr;
mod bucket;
mod cfg;
mod common;
mod dbuf;
mod engine;
mod error;
mod hash;
mod list;
mod meta;
mod ticket;

pub use cfg::TurboCfg;
pub use error::{TurboError, TurboResult};
pub use list::{TurboList, TurboListIter};
pub use ticket::TurboTicket;

use engine::Engine;

/// TurboFox is an embedded key-value storage engine engineered for on-device persistence
///
/// ## Example
///
/// ```
/// use turbofox::{TurboCfg, TurboFox};
///
/// let dir = tempfile::tempdir().unwrap();
/// let path = dir.path();
/// let cfg = TurboCfg::new(path);
/// let db = TurboFox::create(cfg).unwrap();
///
/// let handle = db.clone();
/// handle.set(b"shared", b"payload").unwrap();
/// assert_eq!(db.get(b"shared").unwrap(), Some(b"payload".to_vec()));
///
/// drop(handle);
/// db.delete().unwrap();
/// ```
#[derive(Debug, Clone)]
pub struct TurboFox {
    engine: Arc<Engine>,
}

impl TurboFox {
    /// Creates and initializes a new [`TurboFox`] database instance on disk
    ///
    /// ## Errors
    ///
    /// Returns [`TurboError::Storage`] if the target file already exists; or if filesystem
    /// allocation fails
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::new(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// assert_eq!(db.total_entries(), 0);
    /// db.delete().unwrap();
    /// ```
    /// Ref: `DESIGN.md` §8.1
    pub fn create(cfg: TurboCfg) -> TurboResult<Self> {
        let engine = Engine::create(cfg)?;
        Ok(Self { engine: Arc::new(engine) })
    }

    /// Opens an existing [`TurboFox`] database instance from disk;
    ///
    /// ## Errors
    ///
    /// Returns [`TurboError::Storage`] if the file does not exist or cannot be accessed; returns
    /// [`TurboError::CorruptedHeader`] if header validation or layout checks fail
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::new(path);
    ///
    /// {
    ///     let db = TurboFox::create(cfg.clone()).unwrap();
    ///     db.set(b"persisted", b"true").unwrap();
    ///     db.sync().unwrap();
    /// }
    ///
    /// let reopened = TurboFox::open(cfg).unwrap();
    /// assert_eq!(reopened.get(b"persisted").unwrap(), Some(b"true".to_vec()));
    /// reopened.delete().unwrap();
    /// ```
    /// Ref: `DESIGN.md` §8.1
    pub fn open(cfg: TurboCfg) -> TurboResult<Self> {
        let engine = Engine::open(cfg)?;
        Ok(Self { engine: Arc::new(engine) })
    }

    /// Inserts a new key-value pair or updates an existing entry
    ///
    /// ## [`TurboTicket`]
    ///
    /// This call returns a [`TurboTicket`] representing write acknowledgement and durability state
    ///
    /// ## Errors
    ///
    /// Returns [`TurboError::PayloadTooLarge`] if key or value exceeds maximum payload limits; or
    /// [`TurboError::Storage`] if physical storage expansion fails
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::new(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// let ticket = db.set(b"key_a", b"val_a").unwrap();
    /// assert!(ticket.epoch() > 0);
    /// assert_eq!(db.get(b"key_a").unwrap(), Some(b"val_a".to_vec()));
    ///
    /// db.delete().unwrap();
    /// ```
    /// Ref: `DESIGN.md` §7.2
    #[inline(always)]
    pub fn set(&self, key: &[u8], val: &[u8]) -> TurboResult<TurboTicket> {
        self.engine.set(key, val)
    }

    /// Retrieves the value payload for the specified key; returns `Ok(None)` if key does not exist
    /// or has been deleted
    ///
    /// ## Errors
    ///
    /// Returns [`TurboError::CorruptedPayload`] if data buffer verification or CRC integrity fails
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::new(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// assert_eq!(db.get(b"missing").unwrap(), None);
    /// db.set(b"found", b"content").unwrap();
    /// assert_eq!(db.get(b"found").unwrap(), Some(b"content".to_vec()));
    ///
    /// db.delete().unwrap();
    /// ```
    /// Ref: `DESIGN.md` §7.1
    #[inline(always)]
    pub fn get(&self, key: &[u8]) -> TurboResult<Option<Vec<u8>>> {
        self.engine.get(key)
    }

    /// Deletes an existing key-value or [`TurboList`] entry from the database; returns `Ok(true)`
    /// if entry was found and removed or `Ok(false)` if entry did not exist
    ///
    /// ## Errors
    ///
    /// Returns [`TurboError::Storage`] if metadata writeback fails during deletion
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::new(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// db.set(b"temp_key", b"temp_val").unwrap();
    /// assert!(db.del(b"temp_key").unwrap());
    /// assert!(!db.del(b"temp_key").unwrap());
    ///
    /// db.delete().unwrap();
    /// ```
    /// Ref: `DESIGN.md` §7.3
    #[inline(always)]
    pub fn del(&self, key: &[u8]) -> TurboResult<bool> {
        self.engine.del(key)
    }

    /// Accesses or creates a 1-to-N [`TurboList`] instance anchored at the given key; allows
    /// sequential append, pop, and index-based random access
    ///
    /// ## Examples
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::new(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// let list = db.list(b"sensor_log");
    /// list.append(b"reading_0").unwrap();
    /// list.append(b"reading_1").unwrap();
    ///
    /// assert_eq!(list.len().unwrap(), 2);
    /// assert_eq!(list.first().unwrap(), Some(b"reading_0".to_vec()));
    /// assert_eq!(list.last().unwrap(), Some(b"reading_1".to_vec()));
    ///
    /// drop(list);
    /// db.delete().unwrap();
    /// ```
    /// Ref: `DESIGN.md` §7.4
    #[inline(always)]
    pub fn list(&self, key: &[u8]) -> TurboList {
        TurboList::new(Arc::clone(&self.engine), key.to_vec())
    }

    /// Returns the total count of active key-value and list entries present in the database
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::new(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// assert_eq!(db.total_entries(), 0);
    /// db.set(b"k1", b"v1").unwrap();
    /// db.set(b"k2", b"v2").unwrap();
    /// assert_eq!(db.total_entries(), 2);
    ///
    /// db.del(b"k1").unwrap();
    /// assert_eq!(db.total_entries(), 1);
    ///
    /// db.delete().unwrap();
    /// ```
    /// Ref: `DESIGN.md` §3.1
    #[inline]
    pub fn total_entries(&self) -> u64 {
        self.engine.total_entries()
    }

    /// Returns the in-memory DataBuf pool slot index currently hinted for allocation;
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::new(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// assert_eq!(db.active_dbuf_hint(), 0x10);
    /// db.delete().unwrap();
    /// ```
    /// Ref: `DESIGN.md` §3.3
    #[inline]
    pub fn active_dbuf_hint(&self) -> u32 {
        self.engine.active_dbuf_hint()
    }

    /// Returns the total number of physical pages currently allocated to the database
    ///
    /// ## Errors
    ///
    /// Returns [`TurboError::Storage`] if reading metadata fails
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::new(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// assert!(db.total_pages().unwrap() > 0);
    ///
    /// db.delete().unwrap();
    /// ```
    /// Ref: `DESIGN.md` §3.1
    #[inline]
    pub fn total_pages(&self) -> TurboResult<u32> {
        self.engine.total_pages()
    }

    /// Returns the remaining physical pages available before reaching maximum database capacity
    ///
    /// ## Errors
    ///
    /// Returns [`TurboError::Storage`] if querying database length fails
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::new(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// assert!(db.remaining_pages().unwrap() > 0);
    ///
    /// db.delete().unwrap();
    /// ```
    /// Ref: `DESIGN.md` §9.1
    #[inline]
    pub fn remaining_pages(&self) -> TurboResult<u32> {
        self.engine.remaining_pages()
    }

    /// Returns the remaining available storage capacity in bytes before reaching maximum limit
    ///
    /// ## Errors
    ///
    /// Returns [`TurboError::Storage`] if querying database length fails
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::new(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// assert!(db.remaining_bytes().unwrap() > 0);
    ///
    /// db.delete().unwrap();
    /// ```
    /// Ref: `DESIGN.md` §9.1
    #[inline]
    pub fn remaining_bytes(&self) -> TurboResult<u64> {
        self.engine.remaining_bytes()
    }

    /// Flushes dirty pages and persists pending metadata and writes to storage
    ///
    /// ## Errors
    ///
    /// Returns [`TurboError::Storage`] if synchronization or metadata writeback fails
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::new(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// db.set(b"key", b"value").unwrap();
    /// assert!(db.sync().is_ok());
    ///
    /// db.delete().unwrap();
    /// ```
    /// Ref: `DESIGN.md` §8.1
    #[inline]
    pub fn sync(&self) -> TurboResult<()> {
        self.engine.sync()
    }

    /// Closes the database and deletes all backing files from disk; fails if other handles exist
    ///
    /// ## Errors
    ///
    /// Returns [`TurboError::ResourceBusy`] if other references or active handles exist;
    /// returns [`TurboError::Storage`] if filesystem deletion fails
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::new(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// assert!(db.delete().is_ok());
    /// ```
    /// Ref: `DESIGN.md` §8.1
    pub fn delete(self) -> TurboResult<()> {
        match Arc::try_unwrap(self.engine) {
            Ok(engine) => engine.delete(),
            Err(_) => Err(TurboError::ResourceBusy(
                "cannot delete database with active concurrent handles",
            )),
        }
    }
}
