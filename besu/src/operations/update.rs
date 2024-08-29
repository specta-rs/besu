use std::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
};

use crate::{Dialect, Driver, Table};

pub struct UpdateOperation {
    pub table: &'static str,
}

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

    // pub fn to_sql(&self) -> (String, Vec<SqlValue>) {
    //     D::Dialect::update(self)
    // }
}

impl<D: Driver, T: Table> IntoFuture for Update<D, T> {
    type Output = Result<Vec<T::Model>, D::Error>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output>>>;

    fn into_future(self) -> Self::IntoFuture {
        let driver = self.driver.clone();
        let (sql, values) = D::Dialect::update::<()>(&self.op);

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
