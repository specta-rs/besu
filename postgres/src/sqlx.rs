use std::marker::PhantomData;

use sqlx::{
    error::BoxDynError,
    postgres::{PgArguments, PgConnectOptions, PgPoolOptions, PgQueryResult, PgRow, PgValueRef},
    Arguments, Executor, Pool, Row,
};

use besu::{DecodeValue, Driver, EncodeArgument};

use crate::Postgres;

/// Postgres driver implementation based on [`sqlx`].
#[derive(Debug, Clone)]
pub struct Sqlx(Pool<sqlx::Postgres>);

impl Sqlx {
    /// Construct a new [`Sqlx`] driver from a URI.
    pub async fn new(uri: impl AsRef<str>) -> Result<Self, sqlx::Error> {
        let uri = uri.as_ref();
        // TODO: Expose these params so the user can override them???
        let opts: PgConnectOptions = uri.parse()?;

        // TODO: Expose these params so the user can override them???
        PgPoolOptions::new().connect_with(opts).await.map(Self)
    }

    /// Get a reference to the inner [`Pool`] of the driver.
    pub fn inner(&self) -> &Pool<sqlx::Postgres> {
        &self.0
    }
}

impl Driver for Sqlx {
    type Error = sqlx::Error;
    type Dialect = Postgres;
    type Row = PgRow;
    type Value<'a> = PgValueRef<'a>;
    type Output = PgQueryResult;

    type Arguments<'a> = EncodeImpl<'a>;
    type ValueDecoder = DecodeImpl;

    fn row_len(row: &Self::Row) -> usize {
        row.len()
    }

    fn get_value(row: &Self::Row, index: usize) -> Result<Self::Value<'_>, Self::Error> {
        row.try_get_raw(index)
    }

    fn error_encoding_arguments(
        err: Box<dyn std::error::Error + Send + Sync + 'static>,
    ) -> Self::Error {
        sqlx::Error::Encode(err)
    }

    fn error_decoding_value(
        err: Box<dyn std::error::Error + Send + Sync + 'static>,
    ) -> Self::Error {
        sqlx::Error::Decode(err)
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
        self.0.execute((sql, Some(args.0))).await
    }
}

pub struct DecodeImpl;

impl<T: for<'q> sqlx::Decode<'q, sqlx::Postgres>> DecodeValue<Sqlx, T> for DecodeImpl {
    fn decode<'q>(value: PgValueRef<'q>) -> Result<T, BoxDynError> {
        <T as sqlx::Decode<'q, sqlx::Postgres>>::decode(value)
    }
}

#[derive(Default)]
pub struct EncodeImpl<'a>(PgArguments, PhantomData<&'a ()>);

impl<'a, T: for<'q> sqlx::Encode<'q, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>>
    EncodeArgument<'a, T> for EncodeImpl<'a>
{
    fn encode(&mut self, value: T) -> Result<(), BoxDynError>
    where
        T: 'a,
    {
        self.0.add(value)
    }
}
