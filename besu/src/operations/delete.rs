use std::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
};

use crate::{Dialect, Driver, Table};

pub struct DeleteOperation {
    pub table: &'static str,
}

pub struct Delete<D: Driver, T> {
    driver: D,
    op: DeleteOperation,
    phantom: PhantomData<T>,
}

impl<D: Driver, T: Table> Delete<D, T> {
    pub(crate) fn new(driver: D) -> Self {
        Self {
            driver,
            op: DeleteOperation { table: T::NAME },
            phantom: PhantomData,
        }
    }

    // pub fn to_sql(&self) -> (String, Vec<SqlValue>) {
    //     D::Dialect::delete(self.op)
    // }
}

impl<D: Driver, T: Table> IntoFuture for Delete<D, T> {
    type Output = Result<Vec<T>, D::Error>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output>>>;

    fn into_future(self) -> Self::IntoFuture {
        let driver = self.driver.clone();
        let (sql, values) = D::Dialect::delete::<()>(&self.op);

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
