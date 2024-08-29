use std::{error, future::Future};

use crate::Dialect;

/// A driver implements the database execution logic.
pub trait Driver: Clone + 'static {
    /// Represents the error type the driver can return.
    type Error: error::Error + Send + Sync + 'static;
    /// The dialect of the database.
    type Dialect: Dialect;

    /// A single row of the result set.
    type Row;
    /// A single value in a row.
    type Value<'a>;
    /// The output of a query execution that doesn't return rows.
    type Output;

    /// A collection of arguments.
    type Arguments<'a>: Default;
    /// The value decoder for the driver. Refer to [`DecodeValue`](crate::DecodeValue) for more information.
    type ValueDecoder;

    /// Returns the number of columns in a row.
    fn row_len(row: &Self::Row) -> usize;
    /// Takes a value from the row.
    fn get_value(row: &Self::Row, index: usize) -> Result<Self::Value<'_>, Self::Error>;

    /// Converts an error into the driver's error type when encoding arguments fails.
    fn error_encoding_arguments(err: Box<dyn error::Error + Send + Sync + 'static>) -> Self::Error;
    /// Converts an error into the driver's error type when decoding a value fails.
    fn error_decoding_value(err: Box<dyn error::Error + Send + Sync + 'static>) -> Self::Error;

    /// Executes a query and returns the result set.
    fn query(
        &self,
        sql: &str,
        args: Self::Arguments<'_>,
    ) -> impl Future<Output = Result<Vec<Self::Row>, Self::Error>>;

    /// Executes a query that doesn't return rows.
    fn execute(
        &self,
        sql: &str,
        args: Self::Arguments<'_>,
    ) -> impl Future<Output = Result<Self::Output, Self::Error>>;
}
