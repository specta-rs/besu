use std::error;

use crate::Driver;

/// A trait for decoding a row from a database.
pub trait DecodeRow<D: Driver> {
    /// Decode a row from the database.
    fn decode(row: D::Row) -> Result<Self, Box<dyn error::Error + Send + Sync + 'static>>
    where
        Self: Sized;
}

/// A trait for decoding a value from a database.
pub trait DecodeValue<D: Driver, T> {
    /// Decode a value from the database.
    fn decode<'q>(value: D::Value<'q>) -> Result<T, Box<dyn error::Error + Send + Sync + 'static>>;
}
