//! Error taxonomy and specialized [`Result`] alias for [`crate::TurboFox`]
//!
//! Categorizes disk IO, memory-mapping faults, header corruption, layout mismatches, and resource
//! busy conditions across the engine lifecycle
//!
//! ## Example
//!
//! ```
//! use turbofox::{TurboError, TurboResult};
//!
//! fn check_capacity(size: usize) -> TurboResult<()> {
//!     if size > 0x1000 {
//!         return Err(TurboError::PayloadTooLarge);
//!     }
//!
//!     Ok(())
//! }
//!
//! assert!(check_capacity(0x0100).is_ok());
//! assert!(matches!(check_capacity(0x2000), Err(TurboError::PayloadTooLarge)));
//! ```

/// Specialized [`Result`] type alias for [`crate::TurboFox`] engine operations which returns a
/// [`TurboError`] on failure
///
/// ## Example
///
/// ```
/// use turbofox::{TurboError, TurboResult};
///
/// fn sample_op(succeed: bool) -> TurboResult<u32> {
///     if succeed {
///         Ok(1)
///     } else {
///         Err(TurboError::InvalidMagic)
///     }
/// }
///
/// assert_eq!(sample_op(true).unwrap(), 1);
/// assert!(sample_op(false).is_err());
/// ```
pub type TurboResult<T> = Result<T, TurboError>;

/// [`crate::TurboFox`] engine error variants
///
/// ## Example
///
/// ```
/// use turbofox::TurboError;
///
/// let err = TurboError::InvalidMagic;
/// assert_eq!(err.to_string(), "invalid file magic header");
///
/// let busy = TurboError::ResourceBusy("active reader handles");
/// assert_eq!(busy.to_string(), "resource busy: active reader handles");
/// ```
#[derive(Debug)]
pub enum TurboError {
    /// Low-level memory mapping, paging, or synchronization failures
    Storage {
        /// System error code
        code: u32,
        /// Context description
        context: Box<str>,
    },

    /// Operating system file descriptor IO failures
    Io(std::io::Error),

    /// Corrupted database header metadata
    CorruptedHeader(&'static str),

    /// File header magic bytes do not match expected signature
    InvalidMagic,

    /// Database format layout version mismatch
    VersionMismatch {
        /// Expected layout version
        expected: u32,
        /// Actual layout version found
        found: u32,
    },

    /// Host page size violates power-of-two or minimum size constraints
    InvalidPageSize(u32),

    /// File length is not an exact multiple of the physical page size
    InvalidSize {
        /// Expected size in bytes
        expected: usize,
        /// Actual size found in bytes
        found: usize,
    },

    /// Database root metadata failed integrity validation
    InvalidRootNode,

    /// Resource is currently busy or referenced by active handles
    ResourceBusy(&'static str),

    /// Key type mismatch such as key-value operations on list structures
    InvalidType(&'static str),

    /// Key or value payload exceeds engine storage limits (1004 bytes for KV pairs)
    PayloadTooLarge,

    /// Payload data failed CRC32 validation or offset integrity
    CorruptedPayload(&'static str),

    /// Storage capacity exhausted; cannot allocate additional physical pages
    DatabaseFull,

    /// Trie depth recursion ceiling reached due to hash collision limit
    CollisionLimit,
}

impl From<frozen_core::error::FrozenError> for TurboError {
    #[inline]
    fn from(err: frozen_core::error::FrozenError) -> Self {
        let code = ((err.module as u32) << 0x10) | ((err.domain as u32) << 8) | (err.reason as u32);

        Self::Storage { code, context: err.context }
    }
}

impl From<std::io::Error> for TurboError {
    #[inline]
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl core::fmt::Display for TurboError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Storage { code, context } => {
                write!(f, "storage engine error [0x{:06X}]: {}", code, context)
            }
            Self::Io(err) => write!(f, "io error: {}", err),
            Self::CorruptedHeader(reason) => write!(f, "corrupted header: {}", reason),
            Self::InvalidMagic => write!(f, "invalid file magic header"),
            Self::VersionMismatch { expected, found } => {
                write!(f, "version mismatch: expected {}, found {}", expected, found)
            }
            Self::InvalidPageSize(sz) => write!(f, "invalid page size: {}", sz),
            Self::InvalidSize { expected, found } => {
                write!(f, "invalid file size: expected {}, found {}", expected, found)
            }
            Self::InvalidRootNode => write!(f, "invalid root node discriminator"),
            Self::ResourceBusy(reason) => write!(f, "resource busy: {}", reason),
            Self::InvalidType(reason) => write!(f, "invalid key type: {}", reason),
            Self::PayloadTooLarge => write!(f, "payload exceeds max buffer capacity"),
            Self::CorruptedPayload(reason) => write!(f, "corrupted payload: {}", reason),
            Self::DatabaseFull => write!(f, "database storage capacity exhausted"),
            Self::CollisionLimit => write!(f, "trie depth collision limit exceeded"),
        }
    }
}

impl std::error::Error for TurboError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn ok_error_display_and_source() {
        let err_io =
            TurboError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "file missing"));
        assert!(err_io.to_string().contains("io error: file missing"));
        assert!(err_io.source().is_some());

        let err_storage =
            TurboError::Storage { code: 0x0046_0102, context: Box::from("mmap failure") };
        assert_eq!(err_storage.to_string(), "storage engine error [0x460102]: mmap failure");
        assert!(err_storage.source().is_none());

        assert_eq!(TurboError::InvalidMagic.to_string(), "invalid file magic header");
        assert_eq!(
            TurboError::VersionMismatch { expected: 2, found: 1 }.to_string(),
            "version mismatch: expected 2, found 1"
        );
        assert_eq!(TurboError::InvalidPageSize(0x2000).to_string(), "invalid page size: 8192");
        assert_eq!(
            TurboError::InvalidSize { expected: 0x4000, found: 0x3000 }.to_string(),
            "invalid file size: expected 16384, found 12288"
        );
        assert_eq!(TurboError::InvalidRootNode.to_string(), "invalid root node discriminator");
        assert_eq!(
            TurboError::ResourceBusy("active lock").to_string(),
            "resource busy: active lock"
        );
        assert_eq!(TurboError::InvalidType("list key").to_string(), "invalid key type: list key");
        assert_eq!(TurboError::PayloadTooLarge.to_string(), "payload exceeds max buffer capacity");
        assert_eq!(TurboError::CorruptedPayload("crc").to_string(), "corrupted payload: crc");
        assert_eq!(TurboError::DatabaseFull.to_string(), "database storage capacity exhausted");
        assert_eq!(TurboError::CollisionLimit.to_string(), "trie depth collision limit exceeded");
        assert_eq!(
            TurboError::CorruptedHeader("bad total_slots").to_string(),
            "corrupted header: bad total_slots"
        );
    }

    #[test]
    fn ok_error_from_conversions() {
        let io_err = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let turbo_io = TurboError::from(io_err);
        assert!(matches!(turbo_io, TurboError::Io(_)));

        let frozen_err = frozen_core::error::FrozenError {
            module: 0x46,
            domain: 0x02,
            reason: 0x03,
            context: Box::from("frozen context"),
            suppressed: None,
        };
        let turbo_storage = TurboError::from(frozen_err);
        match turbo_storage {
            TurboError::Storage { code, context } => {
                assert_eq!(code, (0x46 << 0x10) | (0x02 << 8) | 0x03);
                assert_eq!(&*context, "frozen context");
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }
}
