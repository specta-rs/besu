use std::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
};

use crate::{Bruh, Column, Dialect, Driver, Model, Table};

pub struct SelectOperation<V> {
    pub table: &'static str,
    pub where_equals: Vec<(&'static str, V)>,
}

pub struct Select<D: Driver, T> {
    driver: D,
    op: SelectOperation<()>, // TODO: D::Value>,
    phantom: PhantomData<T>,
}

impl<D: Driver, T: Table> Select<D, T> {
    pub(crate) fn new(driver: D) -> Self {
        Self {
            driver,
            op: SelectOperation {
                table: T::NAME,
                where_equals: vec![],
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

    // pub fn to_sql(&self) -> (String, Vec<D::Value>) {
    //     D::Dialect::select(&self.op)
    // }
}

impl<D: Driver, T: Table> IntoFuture for Select<D, T>
where
    // TODO: Should we constrain earlier for better errors????
    T::Model: Bruh<D>,
{
    type Output = Result<Vec<T::Model>, D::Error>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output>>>;

    fn into_future(self) -> Self::IntoFuture {
        let driver = self.driver.clone();
        let (sql, values) = D::Dialect::select(&self.op);

        println!("sql: {}", sql); // TODO

        Box::pin(async move {
            Ok(driver
                .query(&sql, &values)
                .await?
                .into_iter()
                .map(|r| T::Model::decode(r))
                .collect())
        })
    }
}
