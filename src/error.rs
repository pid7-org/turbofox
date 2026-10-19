//! Error taxonomy and result type definitions
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
//!     Ok(())
//! }
//!
//! assert!(check_capacity(0x0100).is_ok());
//! assert!(matches!(check_capacity(0x2000), Err(TurboError::PayloadTooLarge)));
//! ```

/// Specialized [`Result`] type alias for [`TurboFox`] engine operations; returns a [`TurboError`]
/// on failure
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

/// [`TurboFox`] engine error variants
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
    Storage { code: u32, context: Box<str> },

    /// Operating system file descriptor IO failures
    Io(std::io::Error),

    /// Corrupted database header metadata
    CorruptedHeader(&'static str),

    /// File header magic bytes do not match expected signature
    InvalidMagic,

    /// Database format layout version mismatch
    VersionMismatch { expected: u32, found: u32 },

    /// Host page size violates power-of-two or minimum size constraints
    InvalidPageSize(u32),

    /// File length is not an exact multiple of the physical page size
    InvalidSize { expected: usize, found: usize },

    /// Root node virtual page failed type discriminator check
    InvalidRootNode,

    /// Resource is currently busy or referenced by active handles
    ResourceBusy(&'static str),

    /// Key type mismatch such as key-value operations on list structures
    InvalidType(&'static str),

    /// Payload exceeds maximum single DataBuf storage capacity
    PayloadTooLarge,

    /// Payload data failed CRC32 validation or offset integrity
    CorruptedPayload(&'static str),
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

impl From<crate::databuf::DataBufError> for TurboError {
    #[inline]
    fn from(err: crate::databuf::DataBufError) -> Self {
        match err {
            crate::databuf::DataBufError::PayloadTooLarge => Self::PayloadTooLarge,
            crate::databuf::DataBufError::CrcMismatch { .. } => {
                Self::CorruptedPayload("databuf payload crc32 mismatch")
            }
            crate::databuf::DataBufError::InvalidIndex => {
                Self::CorruptedPayload("invalid databuf index")
            }
        }
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
