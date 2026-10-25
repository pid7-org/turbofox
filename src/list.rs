//! 1-to-N list structure anchored by primary key
//!
//! ## Example
//!
//! ```
//! use turbofox::{TurboCfg, TurboFox};
//!
//! let dir = tempfile::tempdir().unwrap();
//! let path = dir.path();
//! let db = TurboFox::create(TurboCfg::new(path)).unwrap();
//! let list = db.list(b"queue");
//!
//! list.append(b"msg_0").unwrap();
//! list.append(b"msg_1").unwrap();
//! assert_eq!(list.len().unwrap(), 2);
//!
//! let mut iter = list.iter().unwrap();
//! assert_eq!(iter.next(), Some(b"msg_0".to_vec()));
//! assert_eq!(iter.next(), Some(b"msg_1".to_vec()));
//! assert_eq!(iter.next(), None);
//!
//! drop(iter);
//! drop(list);
//! db.delete().unwrap();
//! ```

use crate::engine::Engine;
use crate::error::TurboResult;
use crate::ticket::TurboTicket;
use std::sync::Arc;

/// Handle representing an ordered 1-to-N sequence anchored by key in [`crate::TurboFox`]
///
/// Backed by indexed adjacent array descriptor blocks and memory-mapped data buffers
///
/// Cloning the object shares the underlying engine reference and key
///
/// ## Example
///
/// ```
/// use turbofox::{TurboCfg, TurboFox};
///
/// let dir = tempfile::tempdir().unwrap();
/// let path = dir.path();
/// let db = TurboFox::create(TurboCfg::new(path)).unwrap();
///
/// let list = db.list(b"tasks");
/// list.append(b"run").unwrap();
/// assert_eq!(list.pop().unwrap(), Some(b"run".to_vec()));
///
/// drop(list);
/// db.delete().unwrap();
/// ```
#[derive(Debug, Clone)]
pub struct TurboList {
    engine: Arc<Engine>,
    key: Vec<u8>,
}

impl TurboList {
    #[inline(always)]
    pub(crate) fn new(engine: Arc<Engine>, key: Vec<u8>) -> Self {
        Self { engine, key }
    }

    /// Return byte slice of the target key anchoring [`TurboList`]
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let db = TurboFox::create(TurboCfg::new(path)).unwrap();
    /// let list = db.list(b"device:events");
    ///
    /// assert_eq!(list.key(), b"device:events");
    ///
    /// drop(list);
    /// db.delete().unwrap();
    /// ```
    #[inline(always)]
    pub fn key(&self) -> &[u8] {
        &self.key
    }

    /// Append an item payload to the tail of the list
    ///
    /// Internally allocates new descriptor blocks as needed and returns a [`TurboTicket`] for
    /// tracking write persistence
    ///
    /// ## Crash Safe Durability
    ///
    /// Returns a [`TurboTicket`] bound to this append operation's assigned commit epoch
    ///
    /// Internally the entry is commited to memory-mapped descriptor blocks immediately, while the
    /// returned ticket allows tracking for crash safe durability (i.e. persistence on disk)
    ///
    /// Callers can query [`TurboTicket::is_durable`], block on [`TurboTicket::wait`], or force
    /// writeback via [`TurboTicket::force`] for explicit durability guarantee
    ///
    /// Fire-and-forget callers can safely discard the ticket without blocking
    ///
    /// ## Errors
    ///
    /// * [`TurboError::PayloadTooLarge`](crate::TurboError::PayloadTooLarge) if item payload
    ///   exceeds maximum buffer capacity
    /// * [`TurboError::Storage`](crate::TurboError::Storage) if allocating storage descriptor
    ///   blocks fails
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    ///
    /// let db = TurboFox::create(TurboCfg::new(path)).unwrap();
    /// let list = db.list(b"log");
    ///
    /// let ticket = list.append(b"entry").unwrap();
    /// assert!(ticket.epoch() > 0);
    /// assert_eq!(list.len().unwrap(), 1);
    ///
    /// drop(list);
    /// db.delete().unwrap();
    /// ```
    #[inline(always)]
    pub fn append(&self, item: &[u8]) -> TurboResult<TurboTicket> {
        self.engine.list_append(&self.key, item)
    }

    /// Remove the item payload at the tail of the list
    ///
    /// Internally copies bytes into an owned `Vec<u8>` and returns `Ok(None)` if the list is empty
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`](crate::TurboError::Storage) if block deallocation fails
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let db = TurboFox::create(TurboCfg::new(path)).unwrap();
    /// let list = db.list(b"stack");
    ///
    /// list.append(b"bottom").unwrap();
    /// list.append(b"top").unwrap();
    /// assert_eq!(list.pop().unwrap(), Some(b"top".to_vec()));
    /// assert_eq!(list.pop().unwrap(), Some(b"bottom".to_vec()));
    /// assert_eq!(list.pop().unwrap(), None);
    ///
    /// drop(list);
    /// db.delete().unwrap();
    /// ```
    #[inline(always)]
    pub fn pop(&self) -> TurboResult<Option<Vec<u8>>> {
        self.engine.list_pop(&self.key)
    }

    /// Retrieve the first item payload in the list without removing it
    ///
    /// Internally copies bytes into an owned `Vec<u8>` or returns `Ok(None)` if the list contains
    /// zero entries
    ///
    /// ## Errors
    ///
    /// * [`TurboError::CorruptedPayload`](crate::TurboError::CorruptedPayload) if item payload
    ///   CRC32 validation fails
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let db = TurboFox::create(TurboCfg::new(path)).unwrap();
    /// let list = db.list(b"items");
    ///
    /// assert_eq!(list.first().unwrap(), None);
    /// list.append(b"head").unwrap();
    /// assert_eq!(list.first().unwrap(), Some(b"head".to_vec()));
    ///
    /// drop(list);
    /// db.delete().unwrap();
    /// ```
    #[inline(always)]
    pub fn first(&self) -> TurboResult<Option<Vec<u8>>> {
        self.engine.list_first(&self.key)
    }

    /// Retrieve the last item payload in the list without removing it
    ///
    /// Internally copies bytes into an owned `Vec<u8>` or returns `Ok(None)` if the list contains
    /// zero entries
    ///
    /// ## Errors
    ///
    /// * [`TurboError::CorruptedPayload`](crate::TurboError::CorruptedPayload) if item payload
    ///   CRC32 validation fails
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let db = TurboFox::create(TurboCfg::new(path)).unwrap();
    /// let list = db.list(b"items");
    ///
    /// assert_eq!(list.last().unwrap(), None);
    /// list.append(b"tail").unwrap();
    /// assert_eq!(list.last().unwrap(), Some(b"tail".to_vec()));
    ///
    /// drop(list);
    /// db.delete().unwrap();
    /// ```
    #[inline(always)]
    pub fn last(&self) -> TurboResult<Option<Vec<u8>>> {
        self.engine.list_last(&self.key)
    }

    /// Retrieve the item payload at the specified 0-based index and returns `Ok(None)` if index is
    /// out of bounds
    ///
    /// Internally traverses chained adjacent array descriptor blocks to locate the target item slot
    ///
    /// ## Errors
    ///
    /// * [`TurboError::CorruptedPayload`](crate::TurboError::CorruptedPayload) if payload
    ///   traversal encounters corrupted links or bad CRC32 checksums
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let db = TurboFox::create(TurboCfg::new(path)).unwrap();
    /// let list = db.list(b"indexed");
    ///
    /// list.append(b"elem_0").unwrap();
    /// list.append(b"elem_1").unwrap();
    /// assert_eq!(list.get_at(0).unwrap(), Some(b"elem_0".to_vec()));
    /// assert_eq!(list.get_at(1).unwrap(), Some(b"elem_1".to_vec()));
    /// assert_eq!(list.get_at(2).unwrap(), None);
    ///
    /// drop(list);
    /// db.delete().unwrap();
    /// ```
    #[inline(always)]
    pub fn get_at(&self, idx: usize) -> TurboResult<Option<Vec<u8>>> {
        self.engine.list_get_at(&self.key, idx)
    }

    /// Compute total number of item entries in this list by reading allocated descriptor chains
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`](crate::TurboError::Storage) if reading list metadata fails
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let db = TurboFox::create(TurboCfg::new(path)).unwrap();
    /// let list = db.list(b"counted");
    ///
    /// assert_eq!(list.len().unwrap(), 0);
    /// list.append(b"one").unwrap();
    /// assert_eq!(list.len().unwrap(), 1);
    ///
    /// drop(list);
    /// db.delete().unwrap();
    /// ```
    #[inline(always)]
    pub fn len(&self) -> TurboResult<usize> {
        self.engine.list_len(&self.key)
    }

    /// Return `true` if this list contains zero items
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`](crate::TurboError::Storage) if reading list length fails
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let db = TurboFox::create(TurboCfg::new(path)).unwrap();
    /// let list = db.list(b"check_empty");
    ///
    /// assert!(list.is_empty().unwrap());
    /// list.append(b"present").unwrap();
    /// assert!(!list.is_empty().unwrap());
    ///
    /// drop(list);
    /// db.delete().unwrap();
    /// ```
    #[inline(always)]
    pub fn is_empty(&self) -> TurboResult<bool> {
        Ok(self.len()? == 0)
    }

    /// Create an iterator snapshot over all item payloads in the list
    ///
    /// Internally eagerly reads and validates all item payloads into memory
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`](crate::TurboError::Storage) if reading item payloads fails
    /// * [`TurboError::CorruptedPayload`](crate::TurboError::CorruptedPayload) on CRC fault
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    ///
    /// let db = TurboFox::create(TurboCfg::new(path)).unwrap();
    /// let list = db.list(b"iter_list");
    ///
    /// list.append(b"val_a").unwrap();
    /// list.append(b"val_b").unwrap();
    ///
    /// let items: Vec<_> = list.iter().unwrap().collect();
    /// assert_eq!(items, vec![b"val_a".to_vec(), b"val_b".to_vec()]);
    ///
    /// drop(list);
    /// db.delete().unwrap();
    /// ```
    #[inline(always)]
    pub fn iter(&self) -> TurboResult<TurboListIter> {
        let items = self.engine.list_items(&self.key)?;

        Ok(TurboListIter { items, index: 0 })
    }

    /// Remove [`crate::TurboList`] and reclaim all allocated descriptor blocks and payload buffers
    ///
    /// Returns `Ok(true)` if list existed and was cleared, `Ok(false)` otherwise
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`](crate::TurboError::Storage) if block reclamation fails
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let db = TurboFox::create(TurboCfg::new(path)).unwrap();
    /// let list = db.list(b"to_clear");
    ///
    /// list.append(b"data").unwrap();
    /// assert!(list.clear().unwrap());
    /// assert_eq!(list.len().unwrap(), 0);
    ///
    /// drop(list);
    /// db.delete().unwrap();
    /// ```
    #[inline(always)]
    pub fn clear(&self) -> TurboResult<bool> {
        self.engine.list_del(&self.key)
    }
}

/// Iterator snapshot over item payloads in a [`TurboList`]
///
/// Implements [`ExactSizeIterator`] allowing pre-sized allocations and direct index queries
///
/// ## Example
///
/// ```
/// use turbofox::{TurboCfg, TurboFox};
///
/// let dir = tempfile::tempdir().unwrap();
/// let path = dir.path();
///
/// let db = TurboFox::create(TurboCfg::new(path)).unwrap();
/// let list = db.list(b"traverse");
///
/// list.append(b"first").unwrap();
/// list.append(b"second").unwrap();
///
/// let mut iter = list.iter().unwrap();
/// assert_eq!(iter.len(), 2);
/// assert_eq!(iter.next(), Some(b"first".to_vec()));
/// assert_eq!(iter.len(), 1);
/// assert_eq!(iter.next(), Some(b"second".to_vec()));
/// assert_eq!(iter.next(), None);
///
/// drop(iter);
/// drop(list);
/// db.delete().unwrap();
/// ```
#[derive(Debug, Clone)]
pub struct TurboListIter {
    items: Vec<Vec<u8>>,
    index: usize,
}

impl Iterator for TurboListIter {
    type Item = Vec<u8>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.items.len() {
            let item = self.items[self.index].clone();
            self.index += 1;
            Some(item)
        } else {
            None
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let rem = self.items.len().saturating_sub(self.index);
        (rem, Some(rem))
    }
}

impl ExactSizeIterator for TurboListIter {}
