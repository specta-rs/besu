use std::{
    future::{Future, IntoFuture},
    pin::Pin,
};

use crate::Driver;

pub struct Execute<'a, D: Driver> {
    driver: D,
    sql: &'a str,
    arguments: D::Arguments<'a>,
}

impl<'a, D: Driver> Execute<'a, D> {
    pub(crate) fn new(driver: D, sql: &'a str, arguments: D::Arguments<'a>) -> Self {
        Self {
            driver,
            sql,
            arguments,
        }
    }

    // TODO: Allow fetching columns
    // TODO: Allow streaming
}

impl<'a, D: Driver> IntoFuture for Execute<'a, D> {
    type Output = Result<D::Output, D::Error>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        let driver = self.driver.clone();
        Box::pin(async move { driver.execute(&self.sql, self.arguments).await })
    }
}
