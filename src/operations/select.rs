use std::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
    sync::Arc,
};

use crate::{from_value::FromValue, Dialect, Driver, Model, SqlValue};

pub struct Select<D: Driver, T: Model> {
    driver: Arc<D>,
    phantom: PhantomData<T>,
}

impl<D: Driver, T: Model> Select<D, T> {
    pub(crate) fn new(driver: Arc<D>) -> Self {
        Self {
            driver,
            phantom: PhantomData,
        }
    }

    // TODO: where, limit, joins

    // TODO: Take `&self`???
    pub fn to_sql(self) -> (String, Vec<SqlValue>) {
        D::Dialect::select(self)
    }
}

impl<D: Driver, T: Model> IntoFuture for Select<D, T> {
    type Output = Result<Vec<T::Model>, D::Error>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output>>>;

    fn into_future(self) -> Self::IntoFuture {
        let driver = self.driver.clone();
        let (sql, values) = D::Dialect::select(self);

        Box::pin(async move {
            // let result = ;

            // let (sql, values) = self.to_sql()?;

            Ok(driver
                .execute(&sql, &values)
                .await?
                .rows
                .into_iter()
                .flatten()
                .map(|v| T::Model::from_value(v).unwrap())
                .collect())
        })
    }
}
