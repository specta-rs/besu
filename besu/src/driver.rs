use std::{error, future::Future};

use crate::Dialect;

/// A driver implements the database execution logic.
pub trait Driver: Clone + 'static {
    type Error: error::Error;
    type Dialect: Dialect;

    type Row;
    type Value<'a>;
    type Output;

    fn row_len(row: &Self::Row) -> usize;
    fn get_value<'a>(row: &'a Self::Row, index: usize) -> Self::Value<'a>;

    fn query(
        &self,
        sql: &str,
        values: &[()], // TODO: Self::Value
    ) -> impl Future<Output = Result<Vec<Self::Row>, Self::Error>>;

    fn execute(
        &self,
        sql: &str,
        values: &[()], // TODO: Self::Value
    ) -> impl Future<Output = Result<Self::Output, Self::Error>>;
}

pub trait TODO<D: Driver> {
    fn todo(row: D::Row) -> Self;
}
