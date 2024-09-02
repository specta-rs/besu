use std::{borrow::Cow, fmt, marker::PhantomData};

use crate::{DeleteOperation, InsertOperation, SelectOperation, UpdateOperation};

/// A dialect implements the SQL syntax for a specific database kind.
pub trait Dialect {
    // TODO: Do we need this if the whole user is generated in the dialect's code?
    /// Generate the SQL placeholder for a positional parameter.
    fn positional_param(idx: usize) -> Cow<'static, str>;

    /// Generate the SQL query for a `SELECT` operation.
    fn select(op: &SelectOperation) -> (String, Vec<InsertArgumentRef<'_>>);

    /// Generate the SQL query for a `INSERT` operation.
    fn insert<'a>(op: &InsertOperation<'a>) -> (String, Vec<InsertArgumentRef<'a>>);

    /// Generate the SQL query for a `UPDATE` operation.
    fn update<V>(op: &UpdateOperation) -> (String, V);

    /// Generate the SQL query for a `DELETE` operation.
    fn delete<V>(op: &DeleteOperation) -> (String, V);
}

// TODO: Move into insert module or make this more generic and reuse everywhere?
#[derive(Clone, Copy)]
pub struct InsertArgumentRef<'a> {
    // TODO: `pub(crate)` fields or not?
    /// The offset in the array. Which `T` are we pulling from?
    pub(crate) i: usize,
    /// The column index in the struct.
    pub(crate) c: usize,
    /// We implement `Copy` because a field can be used mult
    pub(crate) phantom: PhantomData<&'a ()>,
}

impl<'a> fmt::Debug for InsertArgumentRef<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("InsertArgument")
            .field(&self.i)
            .field(&self.c)
            .finish()
    }
}
