use std::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
};

use crate::{DecodeRow, Dialect, Driver, Table};

#[non_exhaustive]
pub struct SelectOperation {
    pub table: &'static str,
}

pub struct Select<D: Driver, T> {
    driver: D,
    op: SelectOperation,
    phantom: PhantomData<T>,
}

impl<D: Driver, T: Table> Select<D, T> {
    pub(crate) fn new(driver: D) -> Self {
        Self {
            driver,
            op: SelectOperation { table: T::NAME },
            phantom: PhantomData,
        }
    }

    // TODO: Allow using any operator. This is just equals for now + allow column or SQL expression and any order.
    // pub fn _where<C: Column<T>>(mut self, col: C, value: C::Type) -> Self
    // where
    //     C::Type: Into<D::Value>,
    // {
    //     self.op.where_equals.push((C::NAME, value.into()));
    //     self
    // }

    // TODO: limit, offset, cursor, joins

    /// Convert the operation to a raw SQL and values.
    pub fn to_sql(&self) -> (String, D::Arguments<'_>) {
        D::Dialect::select(&self.op)
    }
}

impl<D: Driver, T: Table> IntoFuture for Select<D, T>
where
    // TODO: Should we constrain earlier for better errors????
    T::Model: DecodeRow<D>,
{
    type Output = Result<Vec<T::Model>, D::Error>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output>>>;

    fn into_future(self) -> Self::IntoFuture {
        let driver = self.driver.clone();
        let (sql, arguments) = D::Dialect::select(&self.op);

        Box::pin(async move {
            Ok(driver
                .query(&sql, arguments)
                .await?
                .into_iter()
                .map(|r| T::Model::decode(r))
                .collect())
        })
    }
}
