//! An embedded, memory-mapped key-value engine providing lock-free concurrency and explicit sync
//! barriers
//!
//! ## Key-Value Storage
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
//! ## List Operations
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
pub use common::{MAX_KEY_LEN, MAX_VAL_LEN};
pub use error::{TurboError, TurboResult};
pub use list::{TurboList, TurboListIter};
pub use ticket::TurboTicket;

use engine::Engine;
use std::sync::Arc;

/// An embedded, memory-mapped key-value engine providing lock-free concurrency and explicit sync
/// barriers
///
/// Internally wraps an `Arc<Engine>`; cloning is cheap and shares the underlying memory mapping,
/// hash table, and sync worker thread pool across threads
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
    /// Create and initialize a new [`TurboFox`] instance in the target directory
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if database files already exist in the target directory or if
    ///   initial page allocation and mapping fail
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
    pub fn create(cfg: TurboCfg) -> TurboResult<Self> {
        let engine = Engine::create(cfg)?;

        Ok(Self { engine: Arc::new(engine) })
    }

    /// Open an existing [`TurboFox`] instance from disk
    ///
    /// Internally verifies header magic, layout version, and host system page size constraints
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if the database files do not exist or cannot be accessed
    /// * [`TurboError::CorruptedHeader`] if header validation or layout checks fail
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
    ///
    /// reopened.delete().unwrap();
    /// ```
    pub fn open(cfg: TurboCfg) -> TurboResult<Self> {
        let engine = Engine::open(cfg)?;

        Ok(Self { engine: Arc::new(engine) })
    }

    /// Insert or update a key-value pair
    ///
    /// Internally commits bytes into memory-mapped data buffers and returns a [`TurboTicket`]
    /// tracking background write durability
    ///
    /// Key length must not exceed [`crate::MAX_KEY_LEN`] bytes and value payload must not exceed
    /// [`crate::MAX_VAL_LEN`] bytes
    ///
    /// ## Crash Safe Durability
    ///
    /// Returns a [`TurboTicket`] bound to this write's assigned commit epoch
    ///
    /// Internally write is commited to memory-mapped buffers immediately for lock-free read
    /// availability, while the ticket allows callers to track when bytes are physically flushed for
    /// crash safe durability
    ///
    /// Callers requiring durability guarantees can call [`TurboTicket::wait`] to block until
    /// persisted, or [`TurboTicket::force`] to trigger an immediate flush
    ///
    /// Fire-and-forget callers can safely discard the ticket without blocking
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if physical storage expansion fails
    /// * [`TurboError::PayloadTooLarge`] if key or value exceeds maximum payload limits
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
    #[inline(always)]
    pub fn set(&self, key: &[u8], val: &[u8]) -> TurboResult<TurboTicket> {
        self.engine.set(key, val)
    }

    /// Retrieve value payload for the given key
    ///
    /// Internally copies bytes from memory-mapped storage into an owned `Vec<u8>` after verifying
    /// payload CRC32 integrity
    ///
    /// Returns `Ok(None)` if key does not exist or has been deleted
    ///
    /// ## Errors
    ///
    /// * [`TurboError::CorruptedPayload`] if payload CRC32 integrity check fails
    /// * [`TurboError::InvalidType`] if the key points to a list structure
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
    ///
    /// db.set(b"found", b"content").unwrap();
    /// assert_eq!(db.get(b"found").unwrap(), Some(b"content".to_vec()));
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline(always)]
    pub fn get(&self, key: &[u8]) -> TurboResult<Option<Vec<u8>>> {
        self.engine.get(key)
    }

    /// Check whether the specified key exists without allocating or copying its value payload
    ///
    /// Internally performs lock-free bucket probing directly in mapped hash tables; significantly
    /// faster than [`TurboFox::get`] when checking for existence
    ///
    /// ## Errors
    ///
    /// * [`TurboError::InvalidType`] if key exists as a list
    /// * [`TurboError::Storage`] if reading memory-mapped pages fails
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
    /// assert!(!db.contains_key(b"probe").unwrap());
    ///
    /// db.set(b"probe", b"data").unwrap();
    /// assert!(db.contains_key(b"probe").unwrap());
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline(always)]
    pub fn contains_key(&self, key: &[u8]) -> TurboResult<bool> {
        self.engine.contains_key(key)
    }

    /// Delete an existing key-value entry or [`TurboList`] entry from the [`TurboFox`]
    ///
    /// Returns `Ok(true)` if entry was found and removed, `Ok(false)` if absent
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if metadata writeback fails during deletion
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
    #[inline(always)]
    pub fn del(&self, key: &[u8]) -> TurboResult<bool> {
        self.engine.del(key)
    }

    /// Accesse or create a 1-to-N [`TurboList`] structure anchored at the given key
    ///
    /// It allows sequential appends, pops, and index-based random access
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
    #[inline(always)]
    pub fn list(&self, key: &[u8]) -> TurboList {
        TurboList::new(Arc::clone(&self.engine), key.to_vec())
    }

    /// Return the total count of active key-value and [`TurboList`] entries present in the
    /// [`TurboFox`]
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
    #[inline]
    pub fn total_entries(&self) -> u64 {
        self.engine.total_entries()
    }

    /// Return `true` if the database contains zero active key-value or [`TurboList`] entries
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
    /// assert!(db.is_empty());
    /// db.set(b"k", b"v").unwrap();
    /// assert!(!db.is_empty());
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.total_entries() == 0
    }

    /// Return the in-memory DataBuf pool slot index currently hinted for allocation
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
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline]
    pub fn active_dbuf_hint(&self) -> u32 {
        self.engine.active_dbuf_hint()
    }

    /// Return the total count of physical memory-mapped pages currently allocated to the database
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if reading metadata fails
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
    #[inline]
    pub fn total_pages(&self) -> TurboResult<u32> {
        self.engine.total_pages()
    }

    /// Return the remaining physical pages available before hitting maximum database capacity
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if querying database mapping length fails
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
    #[inline]
    pub fn remaining_pages(&self) -> TurboResult<u32> {
        self.engine.remaining_pages()
    }

    /// Return the remaining available storage capacity in bytes before reaching capacity limit
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if querying database mapping length fails
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
    #[inline]
    pub fn remaining_bytes(&self) -> TurboResult<u64> {
        self.engine.remaining_bytes()
    }

    /// Perform an explicit flush for crash safe durability
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if synchronization or metadata writeback fails
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
    #[inline]
    pub fn sync(&self) -> TurboResult<()> {
        self.engine.sync()
    }

    /// Close the database and delete all backing files from disk
    ///
    /// The call requires that all clones and child [`TurboList`] handles have been dropped
    ///
    /// ## Errors
    ///
    /// * [`TurboError::ResourceBusy`] if other active references to this instance exist
    /// * [`TurboError::Storage`] if filesystem file deletion fails
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
    pub fn delete(self) -> TurboResult<()> {
        match Arc::try_unwrap(self.engine) {
            Ok(engine) => engine.delete(),
            Err(_) => Err(TurboError::ResourceBusy(
                "cannot delete database with active concurrent handles",
            )),
        }
    }
}
