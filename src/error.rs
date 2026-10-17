//! TurboFox error taxonomy and result type definitions

pub type TurboResult<T> = Result<T, TurboError>;

/// TurboFox engine error variants
#[derive(Debug)]
pub enum TurboError {
    /// Low-level memory mapping, paging, or synchronization failures
    Storage {
        code: u32,
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
        expected: u32,
        found: u32,
    },
    /// Host page size violates power-of-two or minimum size constraints
    InvalidPageSize(u32),
    /// File length is not an exact multiple of the physical page size
    InvalidSize {
        expected: usize,
        found: usize,
    },
    /// Root node virtual page failed type discriminator check
    InvalidRootNode,
    /// Resource is currently busy or referenced by active handles
    ResourceBusy(&'static str),
}

impl From<frozen_core::error::FrozenError> for TurboError {
    #[inline]
    fn from(err: frozen_core::error::FrozenError) -> Self {
        let code = ((err.module as u32) << 0x10)
            | ((err.domain as u32) << 8)
            | (err.reason as u32);
        Self::Storage {
            code,
            context: err.context,
        }
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
