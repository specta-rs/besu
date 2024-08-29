use std::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
};

use crate::{Dialect, Driver, Table};

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

    // pub fn to_sql(&self) -> (String, Vec<SqlValue>) {
    //     D::Dialect::insert(self)
    // }
}

impl<D: Driver, T: Table> IntoFuture for Insert<D, T> {
    type Output = Result<Vec<T::Model>, D::Error>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output>>>;

    fn into_future(self) -> Self::IntoFuture {
        let driver = self.driver.clone();
        let (sql, values) = D::Dialect::insert::<()>(&self.op);

        Box::pin(async move {
            // Ok(driver
            //     .execute(&sql, &values)
            //     .await?
            //     .rows
            //     .into_iter()
            //     .flatten()
            //     .map(|v| T::Model::from_value(v).unwrap())
            //     .collect())
            todo!();
        })
    }
}
