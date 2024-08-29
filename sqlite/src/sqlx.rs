use std::path::Path;

use sqlx::{
    sqlite::{SqliteArguments, SqliteQueryResult, SqliteRow, SqliteValueRef},
    Arguments, Executor, Pool, Row,
};

use besu::{DecodeValue, Driver, EncodeArgument};

use crate::SQLite;

#[derive(Debug, Clone)]
pub struct Sqlx(Pool<sqlx::Sqlite>);

impl Sqlx {
    /// Construct a new [`Sqlx`] driver from a path.
    pub async fn new(path: impl AsRef<Path>) -> Result<Self, sqlx::Error> {
        let path = path.as_ref();
        let mut pool = sqlx::sqlite::SqlitePoolOptions::new();

        // ':memory:' doesn't play nice with the connection pool - https://github.com/launchbadge/sqlx/issues/362#issuecomment-636661146
        if path.to_str() == Some(":memory:") {
            pool = pool
                .max_connections(1)
                .idle_timeout(None)
                .max_lifetime(None);
        }

        pool.connect_with(
            // TODO: Expose these params so the user can override them???
            sqlx::sqlite::SqliteConnectOptions::new()
                .create_if_missing(true)
                .foreign_keys(true)
                .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
                .filename(path),
        )
        .await
        .map(Self)
    }

    /// Get a reference to the inner [`Pool`] of the driver.
    pub fn inner(&self) -> &Pool<sqlx::Sqlite> {
        &self.0
    }
}

impl Driver for Sqlx {
    type Error = sqlx::Error;
    type Dialect = SQLite;
    type Row = SqliteRow;
    type Value<'a> = SqliteValueRef<'a>;
    type Output = SqliteQueryResult;

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

impl<T: for<'q> sqlx::Decode<'q, sqlx::Sqlite>> DecodeValue<Sqlx, T> for DecodeImpl {
    fn decode<'q>(value: SqliteValueRef<'q>) -> T {
        <T as sqlx::Decode<'q, sqlx::Sqlite>>::decode(value).unwrap() // TODO: Error handling
    }
}

#[derive(Default)]
pub struct EncodeImpl<'a>(SqliteArguments<'a>);

impl<'a, T: for<'q> sqlx::Encode<'q, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>> EncodeArgument<'a, T>
    for EncodeImpl<'a>
{
    fn encode(&mut self, value: T)
    where
        T: 'a,
    {
        self.0.add(value).unwrap(); // TODO: Error handling
    }
}
