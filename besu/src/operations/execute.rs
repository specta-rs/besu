use std::{
    error,
    future::{Future, IntoFuture},
    pin::Pin,
};

use crate::Driver;

type Arguments<'a, D> =
    Result<<D as Driver>::Arguments<'a>, Box<dyn error::Error + Send + Sync + 'static>>;

/// Builder for an raw SQL query.
// #[derive(Debug)] // TODO
pub struct Execute<'a, D: Driver> {
    driver: D,
    sql: &'a str,
    arguments: Arguments<'a, D>,
}

impl<'a, D: Driver> Execute<'a, D> {
    pub(crate) fn new(driver: D, sql: &'a str, arguments: Arguments<'a, D>) -> Self {
        Self {
            driver,
            sql,
            arguments,
        }
    }

    // TODO: `execute` vs `query` vs `fetch_one`
    // TODO: Allow streaming
}

impl<'a, D: Driver> IntoFuture for Execute<'a, D> {
    type Output = Result<D::Output, D::Error>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        let driver = self.driver.clone();
        Box::pin(async move {
            driver
                .execute(
                    self.sql,
                    self.arguments.map_err(D::error_encoding_arguments)?,
                )
                .await
        })
    }
}
