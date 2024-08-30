use std::{
    future::{Future, IntoFuture},
    pin::Pin,
};

use crate::{Dialect, Driver};

/// Represents an SQL `INSERT` operation.
#[non_exhaustive]
pub struct InsertOperation<T> {
    /// The name of the table being inserted.
    pub table: &'static str,
    // TODO: Should the `InsertOperation` be generic on this
    pub values: Vec<T>,
}

/// Builder for an SQL `INSERT` operation.
pub struct Insert<D: Driver, T> {
    driver: D,
    op: InsertOperation<T>,
}

impl<D: Driver, T> Insert<D, T> {
    pub(crate) fn new(driver: D, table: &'static str) -> Self {
        Self {
            driver,
            op: InsertOperation {
                table,
                values: Vec::new(),
            },
        }
    }

    /// Insert a single value into the table.
    pub fn value(mut self, value: T) -> Self {
        self.op.values.push(value);
        self
    }

    /// Insert many values into the table.
    pub fn values(mut self, mut value: Vec<T>) -> Self {
        self.op.values.append(&mut value);
        self
    }

    /// Convert the operation to a raw SQL and values.
    pub fn to_sql(&self) -> (String, D::Arguments<'_>) {
        D::Dialect::insert(&self.op)
    }
}

impl<D: Driver, T> IntoFuture for Insert<D, T> {
    type Output = Result<D::Output, D::Error>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output>>>;

    fn into_future(self) -> Self::IntoFuture {
        let driver = self.driver.clone();
        let (sql, arguments) = D::Dialect::insert(&self.op);
        Box::pin(async move { driver.execute(&sql, arguments).await })
    }
}
