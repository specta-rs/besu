use std::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
};

use crate::{Dialect, Driver, Table};

/// Represents an SQL `UPDATE` operation.
#[non_exhaustive]
pub struct UpdateOperation {
    /// The name of the table being updated.
    pub table: &'static str,
}

/// Builder for an SQL `UPDATE` operation.
pub struct Update<D: Driver, T> {
    driver: D,
    op: UpdateOperation,
    phantom: PhantomData<T>,
}

impl<D: Driver, T: Table> Update<D, T> {
    pub(crate) fn new(driver: D) -> Self {
        Self {
            driver,
            op: UpdateOperation { table: T::NAME },
            phantom: PhantomData,
        }
    }

    /// Convert the operation to a raw SQL and values.
    pub fn to_sql(&self) -> (String, D::Arguments<'_>) {
        D::Dialect::update(&self.op)
    }
}

impl<D: Driver, T: Table> IntoFuture for Update<D, T> {
    type Output = Result<D::Output, D::Error>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output>>>;

    fn into_future(self) -> Self::IntoFuture {
        let driver = self.driver.clone();
        let (sql, arguments) = D::Dialect::update(&self.op);
        Box::pin(async move { driver.execute(&sql, arguments).await })
    }
}
