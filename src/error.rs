// TODO: Convert this to an `enum` and allow matching of common error types.

use std::{borrow::Cow, fmt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    ErrValuesEmpty,

    // TODO: Foreign key constrain, etc

    // TODO: Remove this variant
    Todo(String),
}

impl Error {
    pub fn to_str(&self) -> &str {
        match self {
            Error::ErrValuesEmpty => "values must be called with at least one value",
            Error::Todo(str) => &str,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_str())
    }
}

impl std::error::Error for Error {}
