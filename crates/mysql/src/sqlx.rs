use std::{fmt, path::Path};

use futures::StreamExt;
use sqlx::{
    mysql::{MySqlValue, MySqlValueRef},
    Either, Executor, Pool, Row, Statement, ValueRef,
};

use besu::{Driver, ExecutedQuery, SqlValue};

use crate::MySql;

pub struct Sqlx(Pool<sqlx::MySql>);

impl fmt::Debug for Sqlx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SqlxSqlite").finish() // TODO: Include connection path???
    }
}

impl Sqlx {
    pub async fn new(uri: impl AsRef<str>) -> Result<Self, sqlx::Error> {
        let uri = uri.as_ref();
        // TODO: Expose these params so the user can override them???
        let opts: sqlx::mysql::MySqlConnectOptions = uri.parse()?;

        // TODO: Expose these params so the user can override them???
        sqlx::mysql::MySqlPoolOptions::new()
            .connect_with(opts)
            .await
            .map(Self)
    }
}

impl Driver for Sqlx {
    const NAME: &'static str = "sqlx_mysql";

    type Error = sqlx::Error;
    type Dialect = MySql;

    async fn execute(&self, sql: &str, values: &[SqlValue]) -> Result<ExecutedQuery, Self::Error> {
        // TOOD: Should *we* cache this statement?
        // TODO: Should we use `prepare_with`?
        let statement = self.0.prepare(&sql).await.unwrap();

        let mut query = statement.query();
        for value in values {
            query = match value {
                SqlValue::String(v) => query.bind(v),
                SqlValue::Integer(v) => query.bind(v),
                SqlValue::Boolean(v) => query.bind(v),
                SqlValue::Binary(binary) => query.bind(binary),
                SqlValue::Float(v) => query.bind(v),
                SqlValue::Default => todo!("Not supported by SQLite"),
                SqlValue::Null => query.bind(None as Option<String>), // TODO: Is the `String` bit fine?
            };
        }

        let rows =
            query
                .persistent(false) // TODO: We don't wanna prepare insert mant statements as they can have any number of values
                .fetch_many(&self.0)
                .collect::<Vec<
                    Result<
                        Either<sqlx::mysql::MySqlQueryResult, sqlx::mysql::MySqlRow>,
                        sqlx::Error,
                    >,
                >>()
                .await;

        let mut result: Option<Vec<_>> = None;
        let mut rows_affected = 0;
        let mut last_insert_id = 0;
        for row in rows {
            let row = row?;

            // TODO: Handle more than one result.
            match row {
                Either::Left(result) => {
                    rows_affected = result.rows_affected();
                    last_insert_id = result.last_insert_id();
                }
                Either::Right(row) => {
                    let results = result.get_or_insert_with(Default::default);

                    let mut values = Vec::with_capacity(row.len());
                    for i in 0..row.len() {
                        let y = <MySqlValueRef as ValueRef>::to_owned(&row.try_get_raw(i).unwrap());

                        // TODO: There has gotta be a better way to do this without dropping sqlx compatibility!
                        values.push(if let Ok(s) = sqlx::Value::try_decode::<String>(&y) {
                            SqlValue::String(s)
                        } else if let Ok(s) = sqlx::Value::try_decode::<i32>(&y) {
                            SqlValue::Integer(s as i64) // SAFTEY: Cast to bigger type
                        } else if let Ok(s) = sqlx::Value::try_decode::<bool>(&y) {
                            SqlValue::Boolean(s)
                        } else {
                            todo!();
                        });
                    }

                    results.push(values);
                }
            }
        }

        Ok(ExecutedQuery {
            rows: result,
            rows_affected,
            last_insert_id: last_insert_id as i64, // TODO: This cast is probs bad
        })
    }
}
