use std::marker::PhantomData;

use sqlx::{
    mysql::{
        MySqlArguments, MySqlConnectOptions, MySqlPoolOptions, MySqlQueryResult, MySqlRow,
        MySqlValueRef,
    },
    Arguments, Executor, Pool, Row,
};

use besu::{DecodeValue, Driver, EncodeArgument};

use crate::MySQL;

#[derive(Debug, Clone)]
pub struct Sqlx(Pool<sqlx::MySql>);

impl Sqlx {
    /// Construct a new [`Sqlx`] driver from a URI.
    pub async fn new(uri: impl AsRef<str>) -> Result<Self, sqlx::Error> {
        let uri = uri.as_ref();
        // TODO: Expose these params so the user can override them???
        let opts: MySqlConnectOptions = uri.parse()?;

        // TODO: Expose these params so the user can override them???
        MySqlPoolOptions::new().connect_with(opts).await.map(Self)
    }

    /// Get a reference to the inner [`Pool`] of the driver.
    pub fn inner(&self) -> &Pool<sqlx::MySql> {
        &self.0
    }
}

impl Driver for Sqlx {
    type Error = sqlx::Error;
    type Dialect = MySQL;
    type Row = MySqlRow;
    type Value<'a> = MySqlValueRef<'a>;
    type Output = MySqlQueryResult;

    type Arguments<'a> = EncodeImpl<'a>;
    type ValueDecoder = DecodeImpl;

    fn row_len(row: &Self::Row) -> usize {
        row.len()
    }
    fn get_value<'a>(row: &'a Self::Row, index: usize) -> Self::Value<'a> {
        row.try_get_raw(index).unwrap() // TODO: Error handling
    }

    async fn query(
        &self,
        sql: &str,
        args: Self::Arguments<'_>,
    ) -> Result<Vec<Self::Row>, Self::Error> {
        self.0.fetch_all((sql, Some(args.0))).await
    }

    async fn execute(
        &self,
        sql: &str,
        args: Self::Arguments<'_>,
    ) -> Result<Self::Output, Self::Error> {
        Ok(self.0.execute((sql, Some(args.0))).await?)
    }
}

pub struct DecodeImpl;

impl<T: for<'q> sqlx::Decode<'q, sqlx::MySql>> DecodeValue<Sqlx, T> for DecodeImpl {
    fn decode<'q>(value: MySqlValueRef<'q>) -> T {
        <T as sqlx::Decode<'q, sqlx::MySql>>::decode(value).unwrap() // TODO: Error handling
    }
}

#[derive(Default)]
pub struct EncodeImpl<'a>(MySqlArguments, PhantomData<&'a ()>);

impl<'a, T: for<'q> sqlx::Encode<'q, sqlx::MySql> + sqlx::Type<sqlx::MySql>> EncodeArgument<'a, T>
    for EncodeImpl<'a>
{
    fn encode(&mut self, value: T)
    where
        T: 'a,
    {
        self.0.add(value).unwrap(); // TODO: Error handling
    }
}
