use std::path::Path;

use sqlx::{
    sqlite::{SqliteArguments, SqliteQueryResult, SqliteRow, SqliteValueRef},
    Arguments, Executor, Pool, Row,
};

use besu::{DecodeValue, Driver};

use crate::{encode::CompatSqliteArgument, SQLite, SqliteArgument};

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
    type Argument<'a> = SqliteArgument<'a>;
    type Row = SqliteRow;
    type Value<'a> = SqliteValueRef<'a>;
    type Output = SqliteQueryResult;

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
        values: &[Self::Argument<'_>],
    ) -> Result<Vec<Self::Row>, Self::Error> {
        // TODO: This is effectively a `values.clone()` because sqlx doesn't expose the Vec inside of `SqliteArguments`/
        // TODO: We also kinda have to reallocate because `&[T]` yields `&T` and `SqliteArgument: !Clone`.

        // It would be nice if this could optionally take a `Vec` from the user so we can avoid reallocation
        let mut args = SqliteArguments::default();
        args.reserve(values.len(), 0);
        for value in values.into_iter() {
            args.add(CompatSqliteArgument(value)).unwrap(); // TODO: Error handling
        }

        self.0.fetch_all((sql, Some(args))).await
    }

    async fn execute(
        &self,
        sql: &str,
        values: &[Self::Argument<'_>],
    ) -> Result<Self::Output, Self::Error> {
        // TODO: This is effectively a `values.clone()` because sqlx doesn't expose the Vec inside of `SqliteArguments`/
        // TODO: We also kinda have to reallocate because `&[T]` yields `&T` and `SqliteArgument: !Clone`.

        // It would be nice if this could optionally take a `Vec` from the user so we can avoid reallocation
        let mut args = SqliteArguments::default();
        args.reserve(values.len(), 0);
        for value in values.into_iter() {
            args.add(CompatSqliteArgument(value)).unwrap(); // TODO: Error handling
        }

        Ok(self.0.execute((sql, Some(args))).await?)
    }
}

pub struct DecodeImpl;

impl<T: for<'q> sqlx::Decode<'q, sqlx::Sqlite>> DecodeValue<Sqlx, T> for DecodeImpl {
    fn decode<'q>(value: SqliteValueRef<'q>) -> T {
        <T as sqlx::Decode<'q, sqlx::Sqlite>>::decode(value).unwrap() // TODO: Error handling
    }
}
