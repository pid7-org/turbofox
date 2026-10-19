//! Durability ticket abstraction wrapping underlying acknowledgement handle
//!
//! ## Example
//!
//! ```
//! use std::sync::Arc;
//! use turbofox::{Completion, TurboTicket};
//!
//! let completion = Arc::new(Completion::default());
//! let ticket = TurboTicket::from_raw(1, completion.clone());
//!
//! assert_eq!(ticket.epoch(), 1);
//! assert!(!ticket.is_durable());
//!
//! completion.mark_epoch_as_durable(1);
//! assert!(ticket.is_durable());
//! ```

use crate::error::{TurboError, TurboResult};
use frozen_core::ack::AckTicket;
use std::ops::Deref;
use std::sync::Arc;

pub use frozen_core::ack::{Completion, TEpoch};

/// Durability acknowledgement handle tracking write persistence
///
/// ## Example
///
/// ```
/// use std::sync::Arc;
/// use turbofox::{Completion, TurboTicket};
///
/// let completion = Arc::new(Completion::default());
/// let ticket = TurboTicket::from_raw(0x0A, completion);
/// assert_eq!(ticket.epoch(), 0x0A);
/// ```
#[derive(Debug, Clone)]
#[repr(transparent)]
pub struct TurboTicket {
    inner: AckTicket,
}

impl TurboTicket {
    /// Constructs a new [`TurboTicket`] wrapping an [`AckTicket`]
    ///
    /// ## Example
    ///
    /// ```
    /// use frozen_core::ack::AckTicket;
    /// use std::sync::Arc;
    /// use turbofox::{Completion, TurboTicket};
    ///
    /// let completion = Arc::new(Completion::default());
    /// let ack = AckTicket::new(1, completion);
    /// let ticket = TurboTicket::new(ack);
    /// assert_eq!(ticket.epoch(), 1);
    /// ```
    #[inline(always)]
    pub const fn new(inner: AckTicket) -> Self {
        Self { inner }
    }

    /// Constructs a new [`TurboTicket`] from raw epoch and completion primitives while binding
    /// durability tracking to the shared completion state
    ///
    /// ## Example
    ///
    /// ```
    /// use std::sync::Arc;
    /// use turbofox::{Completion, TurboTicket};
    ///
    /// let completion = Arc::new(Completion::default());
    /// let ticket = TurboTicket::from_raw(2, completion);
    /// assert_eq!(ticket.epoch(), 2);
    /// ```
    #[inline(always)]
    pub const fn from_raw(epoch: TEpoch, completion: Arc<Completion>) -> Self {
        Self { inner: AckTicket::new(epoch, completion) }
    }

    /// Reads assigned durability epoch for the ticket
    ///
    /// ## Example
    ///
    /// ```
    /// use std::sync::Arc;
    /// use turbofox::{Completion, TurboTicket};
    ///
    /// let completion = Arc::new(Completion::default());
    /// let ticket = TurboTicket::from_raw(3, completion);
    /// assert_eq!(ticket.epoch(), 3);
    /// ```
    #[inline(always)]
    pub const fn epoch(&self) -> TEpoch {
        self.inner.epoch()
    }

    /// Checks if the ticket is already durable without blocking
    ///
    /// ## Example
    ///
    /// ```
    /// use std::sync::Arc;
    /// use turbofox::{Completion, TurboTicket};
    ///
    /// let completion = Arc::new(Completion::default());
    /// let ticket = TurboTicket::from_raw(1, completion.clone());
    /// assert!(!ticket.is_durable());
    ///
    /// completion.mark_epoch_as_durable(1);
    /// assert!(ticket.is_durable());
    /// ```
    #[inline(always)]
    pub fn is_durable(&self) -> bool {
        self.inner.is_durable()
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
    /// ```
    /// use std::sync::Arc;
    /// use turbofox::{Completion, TurboTicket};
    ///
    /// let completion = Arc::new(Completion::default());
    /// let ticket = TurboTicket::from_raw(1, completion.clone());
    /// completion.mark_epoch_as_durable(1);
    ///
    /// assert_eq!(ticket.wait().unwrap(), 1);
    /// ```
    #[inline]
    pub fn wait(&self) -> TurboResult<TEpoch> {
        self.inner.wait().map_err(TurboError::from)
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
    /// ```
    /// use std::sync::Arc;
    /// use turbofox::{Completion, TurboTicket};
    ///
    /// let completion = Arc::new(Completion::default());
    /// let ticket = TurboTicket::from_raw(1, completion.clone());
    /// completion.mark_epoch_as_durable(1);
    ///
    /// assert_eq!(ticket.force().unwrap(), 1);
    /// ```
    #[inline]
    pub fn force(&self) -> TurboResult<TEpoch> {
        self.inner.force().map_err(TurboError::from)
    }

    /// Reference to the underlying shared [`Completion`] state
    ///
    /// ## Example
    ///
    /// ```
    /// use std::sync::Arc;
    /// use turbofox::{Completion, TurboTicket};
    ///
    /// let completion = Arc::new(Completion::default());
    /// let ticket = TurboTicket::from_raw(1, completion.clone());
    /// assert_eq!(ticket.completion().read_durable_epoch(), 0);
    /// ```
    #[inline(always)]
    pub fn completion(&self) -> &Arc<Completion> {
        self.inner.completion()
    }

    /// Immutable reference to the wrapped [`AckTicket`]
    ///
    /// ## Example
    ///
    /// ```
    /// use std::sync::Arc;
    /// use turbofox::{Completion, TurboTicket};
    ///
    /// let completion = Arc::new(Completion::default());
    /// let ticket = TurboTicket::from_raw(1, completion);
    /// assert_eq!(ticket.inner().epoch(), 1);
    /// ```
    #[inline(always)]
    pub const fn inner(&self) -> &AckTicket {
        &self.inner
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

impl Deref for TurboTicket {
    type Target = AckTicket;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frozen_core::error::{ErrCode, FrozenError};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn ok_ticket_transparent_layout() {
        assert_eq!(std::mem::size_of::<TurboTicket>(), std::mem::size_of::<AckTicket>());
        assert_eq!(std::mem::align_of::<TurboTicket>(), std::mem::align_of::<AckTicket>());
    }

    #[test]
    fn ok_ticket_epoch_and_is_durable() {
        let completion = Arc::new(Completion::default());
        let ticket = TurboTicket::from_raw(0x2A, completion.clone());

        assert_eq!(ticket.epoch(), 0x2A);
        assert!(!ticket.is_durable());

        completion.mark_epoch_as_durable(0x29);
        assert!(!ticket.is_durable());

        completion.mark_epoch_as_durable(0x2A);
        assert!(ticket.is_durable());
    }

    #[test]
    fn ok_ticket_conversions_and_deref() {
        let completion = Arc::new(Completion::default());
        let raw_ticket = AckTicket::new(0x0F, completion);

        let turbo_ticket: TurboTicket = raw_ticket.clone().into();
        assert_eq!(turbo_ticket.epoch(), 0x0F);
        assert_eq!(turbo_ticket.inner().epoch(), 0x0F);
        assert_eq!(turbo_ticket.as_ref().epoch(), 0x0F);

        let ack_via_from = AckTicket::from(turbo_ticket);
        assert_eq!(ack_via_from.epoch(), 0x0F);
    }

    #[test]
    fn ok_ticket_wait_blocks_until_durable() {
        let completion = Arc::new(Completion::default());
        let epoch = completion.increment_current_epoch();
        let ticket = TurboTicket::from_raw(epoch, completion.clone());

        let t = thread::spawn({
            let completion = completion.clone();
            move || {
                thread::sleep(Duration::from_millis(0x14));
                completion.mark_epoch_as_durable(epoch);
                completion.notify_all_listeners();
            }
        });

        assert_eq!(ticket.wait().unwrap(), epoch);
        t.join().unwrap();
    }

    #[test]
    fn err_ticket_wait_translates_to_turbo_error() {
        let completion = Arc::new(Completion::default());
        let epoch = completion.increment_current_epoch();
        let ticket = TurboTicket::from_raw(epoch, completion.clone());

        let err = FrozenError::new(1, 2, ErrCode::new(3, "test"), "sync write fault");

        let t = thread::spawn({
            let completion = completion.clone();
            move || {
                thread::sleep(Duration::from_millis(0x14));
                completion.set_err(err);
                completion.notify_all_listeners();
            }
        });

        let res = ticket.wait();
        assert!(res.is_err());
        match res.unwrap_err() {
            TurboError::Storage { code, context } => {
                assert!(code > 0);
                assert!(context.contains("sync write fault"));
            }
            other => panic!("unexpected error variant: {:?}", other),
        }
        t.join().unwrap();
    }

    #[test]
    fn err_ticket_force_without_trigger_returns_turbo_error() {
        let completion = Arc::new(Completion::default());
        let epoch = completion.increment_current_epoch();
        let ticket = TurboTicket::from_raw(epoch, completion);

        let res = ticket.force();
        assert!(res.is_err());
        match res.unwrap_err() {
            TurboError::Storage { .. } => {}
            other => panic!("expected Storage error variant, got: {:?}", other),
        }
    }
}
