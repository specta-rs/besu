use std::path::Path;

use futures::StreamExt;
use sqlx::{
    sqlite::{SqliteRow, SqliteValue, SqliteValueRef},
    Either, Executor, Pool, Row, Statement,
};

use besu::{Bruh2, Driver};

use crate::{executed_query::ExecutedQuery, SQLite};

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

struct SqlxValue;

impl Driver for Sqlx {
    type Error = sqlx::Error;
    type Dialect = SQLite;
    type Row = SqliteRow;
    type Value<'a> = SqliteValueRef<'a>;
    type Output = ExecutedQuery;

    fn row_len(row: &Self::Row) -> usize {
        row.len()
    }
    fn get_value<'a>(row: &'a Self::Row, index: usize) -> Self::Value<'a> {
        row.try_get_raw(index).unwrap() // TODO: Error handling
    }

    async fn query(&self, sql: &str, values: &[()]) -> Result<Vec<Self::Row>, Self::Error> {
        // TOOD: Should *we* cache this statement?
        // TODO: Should we use `prepare_with`?
        let statement = self.0.prepare(&sql).await.unwrap();

        let mut query = statement.query();
        for value in values {
            // query = match value {
            //     SqlValue::String(v) => query.bind(v),
            //     SqlValue::Integer(v) => query.bind(v),
            //     SqlValue::Boolean(v) => query.bind(v),
            //     SqlValue::Binary(binary) => query.bind(binary),
            //     SqlValue::Float(v) => query.bind(v),
            //     SqlValue::Default => todo!("Not supported by SQLite"),
            //     SqlValue::Null => query.bind(None as Option<String>), // TODO: Is the `String` bit fine?
            // };
        }

        let rows = query
            .persistent(false) // TODO: We don't wanna prepare insert mant statements as they can have any number of values
            .fetch_many(&self.0)
            .collect::<Vec<
                Result<
                    Either<sqlx::sqlite::SqliteQueryResult, sqlx::sqlite::SqliteRow>,
                    sqlx::Error,
                >,
            >>()
            .await;

        // TODO: handle query returning an error????

        Ok(rows
            .into_iter()
            .filter_map(|r| match r {
                Ok(Either::Right(r)) => Some(r),
                _ => None,
            })
            .collect())
    }

    async fn execute(&self, sql: &str, values: &[()]) -> Result<Self::Output, Self::Error> {
        // TOOD: Should *we* cache this statement?
        // TODO: Should we use `prepare_with`?
        let statement = self.0.prepare(&sql).await.unwrap();

        let mut query = statement.query();
        for value in values {
            // query = match value {
            //     SqlValue::String(v) => query.bind(v),
            //     SqlValue::Integer(v) => query.bind(v),
            //     SqlValue::Boolean(v) => query.bind(v),
            //     SqlValue::Binary(binary) => query.bind(binary),
            //     SqlValue::Float(v) => query.bind(v),
            //     SqlValue::Default => todo!("Not supported by SQLite"),
            //     SqlValue::Null => query.bind(None as Option<String>), // TODO: Is the `String` bit fine?
            // };
        }

        let rows = query
            .persistent(false) // TODO: We don't wanna prepare insert mant statements as they can have any number of values
            .fetch_many(&self.0)
            .collect::<Vec<
                Result<
                    Either<sqlx::sqlite::SqliteQueryResult, sqlx::sqlite::SqliteRow>,
                    sqlx::Error,
                >,
            >>()
            .await;

        // let mut result: Option<Vec<_>> = None;
        let mut rows_affected = 0;
        let mut last_insert_id = 0;
        for row in rows {
            // TODO: Handle more than one result.
            match row? {
                Either::Left(result) => {
                    rows_affected = result.rows_affected();
                    last_insert_id = result.last_insert_rowid();
                }
                Either::Right(row) => {
                    // let results = result.get_or_insert_with(Default::default);

                    // let mut values = Vec::with_capacity(row.len());
                    // for i in 0..row.len() {
                    //     let y = row.try_get_raw(i).unwrap().to_owned();

                    //     // TODO: There has gotta be a better way to do this without dropping sqlx compatibility!
                    //     values.push(if let Ok(s) = sqlx::Value::try_decode::<String>(&y) {
                    //         SqlValue::String(s)
                    //     } else if let Ok(s) = sqlx::Value::try_decode::<i32>(&y) {
                    //         SqlValue::Integer(s as i64) // SAFTEY: Cast to bigger type
                    //     } else if let Ok(s) = sqlx::Value::try_decode::<bool>(&y) {
                    //         SqlValue::Boolean(s)
                    //     } else {
                    //         todo!();
                    //     });
                    // }

                    // results.push(values);
                }
            }
        }

        Ok(ExecutedQuery {
            // rows: result,
            rows_affected,
            last_insert_id,
        })
    }
}

// TODO: Implement all the types
impl Bruh2<Sqlx> for i32 {
    fn decode<'a>(row: SqliteValueRef<'a>) -> Self {
        <Self as sqlx::Decode<'a, sqlx::Sqlite>>::decode(row).unwrap() // TODO: Error handling
    }
}

impl Bruh2<Sqlx> for String {
    fn decode<'a>(row: SqliteValueRef<'a>) -> Self {
        <Self as sqlx::Decode<'a, sqlx::Sqlite>>::decode(row).unwrap() // TODO: Error handling
    }
}

impl<T: Bruh2<Sqlx>> Bruh2<Sqlx> for Option<T> {
    fn decode<'a>(row: SqliteValueRef<'a>) -> Self {
        None // TODO: Fix options
             // <Self as sqlx::Decode<'a, sqlx::Sqlite>>::decode(row).unwrap() // TODO: Error handling
    }
}

// TODO: `Box`, `Arc`, etc
