use std::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
};

use crate::{DecodeRow, Dialect, Driver};

/// Represents an SQL `SELECT` operation.
#[non_exhaustive]
pub struct SelectOperation {
    /// The name of the table being selected.
    pub table: &'static str,
    /// The columns to select.
    pub columns: Vec<&'static str>, // TODO: Don't use a `Vec` use a static slice??
}

/// Builder for an SQL `SELECT` operation.
pub struct Select<D: Driver, T> {
    driver: D,
    op: SelectOperation,
    phantom: PhantomData<T>,
}

impl<D: Driver, T> Select<D, T> {
    pub(crate) fn new(driver: D, table: &'static str) -> Self {
        Self {
            driver,
            op: SelectOperation {
                table,
                columns: Vec::new(),
            },
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

    // pub fn left_join<Right: Model, F: Field<Right>>(
    //     &self,
    //     field: F,
    //     value: F::Type,
    // ) -> LeftJoin<M, Right> {
    //     todo!();
    // }

    // pub type RightJoin<T, U> = (T, Option<U>);
    // pub type LeftJoin<T, U> = (Option<T>, U);
    // pub type InnerJoin<T, U> = (T, U);
    // pub type FullJoin<T, U> = (Option<T>, Option<U>);

    /// Convert the operation to a raw SQL and values.
    pub fn to_sql(&self) -> (String, D::Arguments<'_>) {
        D::Dialect::select(&self.op)
    }
}

impl<D: Driver, T> IntoFuture for Select<D, T>
where
    T: DecodeRow<D>,
{
    type Output = Result<Vec<T>, D::Error>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output>>>;

    fn into_future(self) -> Self::IntoFuture {
        let driver = self.driver.clone();
        let (sql, arguments) = D::Dialect::select(&self.op);

        Box::pin(async move {
            driver
                .query(&sql, arguments)
                .await?
                .into_iter()
                .map(|r| T::decode(r))
                .collect::<Result<Vec<_>, _>>()
                .map_err(D::error_decoding_value)
        })
    }
}
