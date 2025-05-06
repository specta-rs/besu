use std::error;

use crate::{Dialect, Encode};

/// A driver implements the database execution logic.
pub trait Driver: Sized {
    // /// Represents the error type the driver can return.
    // type Error: error::Error + Send + Sync + 'static;
    /// The dialect of the database.
    type Dialect: Dialect;

    type Value; // Encode<Self>

    // TODO: Should this take in `Sql<D, ()>` instead or not???
    fn execute(&self, sql: String, arguments: Vec<Self::Value>);
}
