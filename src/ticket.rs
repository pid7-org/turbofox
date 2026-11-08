//! Write durability ticket bound to a monotonically increasing commit epoch
//!
//! Tracks asynchronous page writebacks driven by background sync threads or explicit flushes
//!
//! ## Example
//!
//! ```
//! use turbofox::{TurboCfg, TurboFox};
//!
//! let dir = tempfile::tempdir().unwrap();
//! let path = dir.path();
//! let db = TurboFox::create(TurboCfg::manual(path)).unwrap();
//! let ticket = db.set(b"key", b"value").unwrap();
//! db.sync().unwrap();
//!
//! assert!(ticket.epoch() > 0x00);
//! assert!(ticket.wait().is_ok());
//!
//! db.delete().unwrap();
//! ```

use crate::error::{TurboError, TurboResult};
use frozen_core::ack::AckTicket;

/// Durability token tracking write persistence across commit epochs
///
/// Wraps an underlying [`AckTicket`]; safe to clone or pass across threads to monitor writeback
/// completion
///
/// Durability barriers are epoch-cumulative; establishing durability for epoch $E$ mathematically
/// guarantees all mutations with epochs $\le E$ are committed to physical media
///
/// ## Example
///
/// ```
/// use turbofox::{TurboCfg, TurboFox};
///
/// let dir = tempfile::tempdir().unwrap();
/// let path = dir.path();
/// let db = TurboFox::create(TurboCfg::manual(path)).unwrap();
///
/// let ticket = db.set(b"key", b"value").unwrap();
/// assert!(ticket.epoch() > 0x00);
///
/// db.delete().unwrap();
/// ```
#[derive(Debug, Clone)]
#[repr(transparent)]
pub struct TurboTicket {
    inner: AckTicket,
}

impl TurboTicket {
    /// Construct a new [`TurboTicket`] wrapping an [`AckTicket`]
    #[inline(always)]
    pub(crate) const fn new(inner: AckTicket) -> Self {
        Self { inner }
    }

    /// Return the monotonically increasing durability epoch assigned to the write operation
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    ///
    /// let db = TurboFox::create(TurboCfg::manual(path)).unwrap();
    /// let ticket = db.set(b"key", b"value").unwrap();
    ///
    /// assert!(ticket.epoch() > 0x00);
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline(always)]
    pub const fn epoch(&self) -> u64 {
        self.inner.epoch()
    }

    /// Check whether this write and all prior writes have been flushed to physical storage
    ///
    /// Performs a non-blocking atomic comparison between the engine's confirmed durable epoch
    /// and this ticket's assigned epoch; returns `true` if durable without parking the caller
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    ///
    /// let db = TurboFox::create(TurboCfg::manual(path)).unwrap();
    /// let ticket = db.set(b"key", b"value").unwrap();
    /// let _ = ticket.is_durable();
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline(always)]
    pub fn is_durable(&self) -> bool {
        self.inner.is_durable()
    }

    /// Block until this write and all prior writes are flushed to disk
    ///
    /// Parks the calling thread on an internal condition variable until the background sync worker
    /// or an explicit flush reaches this ticket's epoch; returns confirmed durable epoch
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if the background flush worker encounters a writeback fault
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let db = TurboFox::create(TurboCfg::manual(path)).unwrap();
    /// let ticket = db.set(b"key", b"value").unwrap();
    /// db.sync().unwrap();
    /// assert!(ticket.wait().is_ok());
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline]
    pub fn wait(&self) -> TurboResult<u64> {
        self.inner.wait().map_err(TurboError::from)
    }

    /// Force an immediate disk flush and block until this write is durable
    ///
    /// Unlike [`TurboTicket::wait`] which waits passively for the periodic background worker,
    /// `force` triggers an immediate `msync` sequence to persist dirty pages without delay
    ///
    /// Returns immediately if this write is already durable; otherwise blocks until disk
    /// synchronization completes and returns confirmed durable epoch
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if sync trigger or flush execution fails
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    ///
    /// let db = TurboFox::create(TurboCfg::manual(path)).unwrap();
    /// let ticket = db.set(b"key", b"value").unwrap();
    ///
    /// assert!(ticket.force().is_ok());
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline]
    pub fn force(&self) -> TurboResult<u64> {
        self.inner.force().map_err(TurboError::from)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TurboCfg, TurboFox};
    use tempfile::tempdir;

    #[test]
    fn ok_ticket_traits_and_conversions() {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path());
        let db = TurboFox::create(cfg).unwrap();

        let ticket = db.set(b"k1", b"v1").unwrap();
        assert!(ticket.epoch() > 0);

        let cloned = ticket.clone();
        assert_eq!(cloned.epoch(), ticket.epoch());
        assert!(format!("{:?}", ticket).contains("TurboTicket"));

        let ack: AckTicket = ticket.clone().into();
        assert_eq!(ack.epoch(), ticket.epoch());

        let converted = TurboTicket::from(ack);
        assert_eq!(converted.epoch(), ticket.epoch());
        assert_eq!(converted.as_ref().epoch(), ticket.epoch());

        assert!(converted.force().is_ok());
        assert!(converted.is_durable());
        assert_eq!(converted.wait().unwrap(), converted.epoch());

        db.delete().unwrap();
    }

    #[test]
    fn ok_ticket_concurrency_and_cloning() {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path());
        let db = TurboFox::create(cfg).unwrap();

        let ticket = db.set(b"thread_key", b"thread_val").unwrap();
        let epoch = ticket.epoch();
        let mut handles = Vec::new();

        for _ in 0..4 {
            let t = ticket.clone();
            handles.push(std::thread::spawn(move || {
                assert_eq!(t.epoch(), epoch);
                assert!(t.force().is_ok());
                assert!(t.is_durable());
                assert_eq!(t.wait().unwrap(), epoch);
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        db.delete().unwrap();
    }
}
