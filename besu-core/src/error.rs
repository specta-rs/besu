use alloc::string::String;
use core::fmt;

/// The kind of failure that can occur while building or running a query.
///
/// Drivers surface their own error type; they convert these core errors into it via
/// [`Driver::error`](crate::Driver::error).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// A value could not be represented by the target dialect.
    ///
    /// The classic example is a `u64` larger than `i64::MAX` being sent to SQLite, which
    /// only has signed 64-bit integers.
    Encode {
        /// The value that could not be encoded, rendered for humans.
        value: String,
        /// Why it could not be encoded.
        reason: String,
    },
    /// A row value could not be decoded into the requested Rust type.
    Decode {
        /// The zero-based column index that failed.
        column: usize,
        /// Why the value could not be decoded.
        reason: String,
    },
    /// The driver does not support the requested feature.
    Unsupported(String),
    /// A query was built in a way that cannot produce valid SQL, for example an empty
    /// `INSERT` or a `RETURNING` clause on a dialect that has no such thing.
    InvalidQuery(String),
}

impl Error {
    /// Create an [`Error::Encode`].
    pub fn encode(value: impl fmt::Display, reason: impl Into<String>) -> Self {
        Self::Encode {
            value: value.to_string(),
            reason: reason.into(),
        }
    }

    /// Create an [`Error::Decode`].
    pub fn decode(column: usize, reason: impl Into<String>) -> Self {
        Self::Decode {
            column,
            reason: reason.into(),
        }
    }

    /// Create an [`Error::Unsupported`].
    pub fn unsupported(feature: impl Into<String>) -> Self {
        Self::Unsupported(feature.into())
    }

    /// Create an [`Error::InvalidQuery`].
    pub fn invalid_query(reason: impl Into<String>) -> Self {
        Self::InvalidQuery(reason.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encode { value, reason } => {
                write!(f, "cannot encode value `{value}`: {reason}")
            }
            Self::Decode { column, reason } => {
                write!(f, "cannot decode column {column}: {reason}")
            }
            Self::Unsupported(feature) => write!(f, "unsupported: {feature}"),
            Self::InvalidQuery(reason) => write!(f, "invalid query: {reason}"),
        }
    }
}

impl std::error::Error for Error {}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&alloc::string::ToString::to_string(self))
    }
}
