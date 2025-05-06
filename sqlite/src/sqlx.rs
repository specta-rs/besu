use std::{error, path::Path};

use sqlx::{
    error::BoxDynError,
    sqlite::{SqliteArguments, SqliteQueryResult, SqliteRow, SqliteValueRef},
    Arguments, Executor, Pool, Row,
};

use besu::Driver;

use crate::SQLite;

/// SQLite driver implementation based on [`sqlx`].
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
    //     type Row = SqliteRow;
    //     type Value<'a> = SqliteValueRef<'a>;
    //     type Output = SqliteQueryResult;

    //     type Arguments<'a> = SqliteArguments<'a>;
    //     type ValueDecoder = DecodeImpl;

    //     fn row_len(row: &Self::Row) -> usize {
    //         row.len()
    //     }

    //     fn get_value(row: &Self::Row, index: usize) -> Result<Self::Value<'_>, Self::Error> {
    //         row.try_get_raw(index)
    //     }

    //     fn error_encoding_arguments(
    //         err: Box<dyn std::error::Error + Send + Sync + 'static>,
    //     ) -> Self::Error {
    //         sqlx::Error::Encode(err)
    //     }

    //     fn error_decoding_value(
    //         err: Box<dyn std::error::Error + Send + Sync + 'static>,
    //     ) -> Self::Error {
    //         sqlx::Error::Decode(err)
    //     }

    //     async fn query(
    //         &self,
    //         sql: &str,
    //         args: Self::Arguments<'_>,
    //     ) -> Result<Vec<Self::Row>, Self::Error> {
    //         self.0.fetch_all((sql, Some(args))).await
    //     }

    //     async fn execute(
    //         &self,
    //         sql: &str,
    //         args: Self::Arguments<'_>,
    //     ) -> Result<Self::Output, Self::Error> {
    //         self.0.execute((sql, Some(args))).await
    //     }
    // }

    // pub struct DecodeImpl;

    // impl<T: for<'q> sqlx::Decode<'q, sqlx::Sqlite>> DecodeValue<Sqlx, T> for DecodeImpl {
    //     fn decode<'q>(value: SqliteValueRef<'q>) -> Result<T, BoxDynError> {
    //         <T as sqlx::Decode<'q, sqlx::Sqlite>>::decode(value)
    //     }
    // }

    // impl<'a, T: sqlx::Encode<'a, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite> + 'a> EncodeArgument<'a, T>
    //     for Sqlx
    // {
    //     fn encode(
    //         arguments: &mut Self::Arguments<'a>,
    //         value: T,
    //     ) -> Result<(), Box<dyn error::Error + Send + Sync + 'static>> {
    //         arguments.add(value)
    //     }
}
