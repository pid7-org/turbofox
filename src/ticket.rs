//! Durability ticket abstraction wrapping underlying acknowledgement handle to track or manage
//! durability of write operations
//!
//! ## Example
//!
//! ```no_run
//! use turbofox::{TurboCfg, TurboFox};
//!
//! let dir = tempfile::tempdir().unwrap();
//! let path = dir.path().join("ticket_mod.db");
//! let db = TurboFox::create(TurboCfg::new(path)).unwrap();
//! let ticket = db.set(b"key", b"value").unwrap();
//! db.sync().unwrap();
//!
//! assert!(ticket.epoch() > 0);
//! assert!(ticket.wait().is_ok());
//!
//! db.delete().unwrap();
//! ```

use crate::error::TurboResult;
use frozen_core::ack::AckTicket;

/// Durability acknowledgement handle tracking write persistence
///
/// ## Example
///
/// ```no_run
/// use turbofox::{TurboCfg, TurboFox};
///
/// let dir = tempfile::tempdir().unwrap();
/// let path = dir.path().join("ticket_type.db");
/// let db = TurboFox::create(TurboCfg::new(path)).unwrap();
///
/// let ticket = db.set(b"key", b"value").unwrap();
/// assert!(ticket.epoch() > 0);
///
/// db.delete().unwrap();
/// ```
#[derive(Debug, Clone)]
#[repr(transparent)]
pub struct TurboTicket {
    inner: AckTicket,
}

impl TurboTicket {
    /// Constructs a new [`TurboTicket`] wrapping an [`AckTicket`]
    #[inline(always)]
    pub(crate) const fn new(inner: AckTicket) -> Self {
        Self { inner }
    }

    /// Reads assigned durability epoch for the ticket
    ///
    /// ## Example
    ///
    /// ```no_run
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path().join("ticket_epoch.db");
    /// let db = TurboFox::create(TurboCfg::new(path)).unwrap();
    /// let ticket = db.set(b"key", b"value").unwrap();
    /// assert!(ticket.epoch() > 0);
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline(always)]
    pub const fn epoch(&self) -> u64 {
        todo!()
    }

    /// Checks if the ticket is already durable without blocking
    ///
    /// ## Example
    ///
    /// ```no_run
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path().join("ticket_durable.db");
    /// let db = TurboFox::create(TurboCfg::new(path)).unwrap();
    /// let ticket = db.set(b"key", b"value").unwrap();
    /// let _ = ticket.is_durable();
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline(always)]
    pub fn is_durable(&self) -> bool {
        todo!()
    }

    /// Blocks the current thread until the ticket becomes durable; translates low-level durability
    /// failure into [`TurboError`]
    ///
    /// ## Errors
    ///
    /// Returns [`TurboError::Storage`] if sync worker reports a writeback failure
    ///
    /// ## Example
    ///
    /// ```no_run
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path().join("ticket_wait.db");
    /// let db = TurboFox::create(TurboCfg::new(path)).unwrap();
    /// let ticket = db.set(b"key", b"value").unwrap();
    /// db.sync().unwrap();
    /// assert!(ticket.wait().is_ok());
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline]
    pub fn wait(&self) -> TurboResult<u64> {
        todo!()
    }

    /// Forces synchronization for this ticket and blocks until durable; while translating low-level
    /// durability failure into [`TurboError`]
    ///
    /// ## Errors
    ///
    /// Returns [`TurboError::Storage`] if sync trigger or flush execution fails
    ///
    /// ## Example
    ///
    /// ```no_run
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path().join("ticket_force.db");
    /// let db = TurboFox::create(TurboCfg::new(path)).unwrap();
    /// let ticket = db.set(b"key", b"value").unwrap();
    /// assert!(ticket.force().is_ok());
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline]
    pub fn force(&self) -> TurboResult<u64> {
        todo!()
    }
}

impl From<AckTicket> for TurboTicket {
    #[inline(always)]
    fn from(inner: AckTicket) -> Self {
        Self { inner }
    }
}

impl From<TurboTicket> for AckTicket {
    #[inline(always)]
    fn from(ticket: TurboTicket) -> Self {
        ticket.inner
    }
}

impl AsRef<AckTicket> for TurboTicket {
    #[inline(always)]
    fn as_ref(&self) -> &AckTicket {
        &self.inner
    }
}
