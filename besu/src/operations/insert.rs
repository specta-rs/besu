use std::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
};

use crate::{Dialect, Driver, Table};

#[non_exhaustive]
pub struct InsertOperation {
    pub table: &'static str,
}

pub struct Insert<D: Driver, T> {
    driver: D,
    op: InsertOperation,
    phantom: PhantomData<T>,
}

impl<D: Driver, T: Table> Insert<D, T> {
    pub(crate) fn new(driver: D) -> Self {
        Self {
            driver,
            op: InsertOperation { table: T::NAME },
            phantom: PhantomData,
        }
    }

    // pub fn value(self) -> Self {
    //     self
    // }

    // pub fn values(self) -> Self {
    //     self
    // }

    /// Convert the operation to a raw SQL and values.
    pub fn to_sql(&self) -> (String, D::Arguments<'_>) {
        D::Dialect::insert(&self.op)
    }
}

impl<D: Driver, T: Table> IntoFuture for Insert<D, T> {
    type Output = Result<D::Output, D::Error>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output>>>;

    fn into_future(self) -> Self::IntoFuture {
        let driver = self.driver.clone();
        let (sql, arguments) = D::Dialect::insert(&self.op);
        Box::pin(async move { driver.execute(&sql, arguments).await })
    }
}
