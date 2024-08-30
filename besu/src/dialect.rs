use std::borrow::Cow;

use crate::{DeleteOperation, InsertOperation, SelectOperation, UpdateOperation};

/// A dialect implements the SQL syntax for a specific database kind.
pub trait Dialect {
    /// Generate the SQL placeholder for a positional parameter.
    fn positional_param(idx: usize) -> Cow<'static, str>;

    /// Generate the SQL query for a `SELECT` operation.
    fn select<V: Default>(op: &SelectOperation) -> (String, V);

    /// Generate the SQL query for a `INSERT` operation.
    fn insert<V: Default, T>(op: &InsertOperation<T>) -> (String, V);

    /// Generate the SQL query for a `UPDATE` operation.
    fn update<V>(op: &UpdateOperation) -> (String, V);

    /// Generate the SQL query for a `DELETE` operation.
    fn delete<V>(op: &DeleteOperation) -> (String, V);
}
