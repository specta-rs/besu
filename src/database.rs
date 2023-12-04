use std::{borrow::Cow, fmt, path::Path};

use futures::stream::StreamExt;
use sqlx::{Either, Executor, Row, Statement, ValueRef};

use crate::{
    queries::{DeleteQuery, InsertQuery, SelectQuery, UpdateQuery},
    schema::Table,
    Error, ExecutedQuery, SqlValue,
};

// Sqlx's `Any` connector provides basically no options so we do this. This will also future proof for non-sqlx connectors.
#[derive(Clone)]
pub(crate) enum Connection {
    #[cfg(feature = "sqlite")]
    Sqlite(sqlx::Pool<sqlx::Sqlite>),
}

pub type LoggerFn = fn(&str) -> ();

#[derive(Clone)]
pub struct Database {
    logger: LoggerFn,
    pub(crate) positional_param: fn(usize) -> Cow<'static, str>,
    conn: Connection,
}

impl fmt::Debug for Database {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Database")
            .field(match self.conn {
                #[cfg(feature = "sqlite")]
                Connection::Sqlite(_) => &"SQLite",
            })
            .finish()
    }
}

impl Database {
    pub async fn new(path: impl AsRef<Path>) -> Result<Self, sqlx::Error> {
        Self::new_with_logger(path, |_| ()).await
    }

    pub async fn new_with_logger(
        path: impl AsRef<Path>,
        logger: LoggerFn,
    ) -> Result<Self, sqlx::Error> {
        #[cfg(not(feature = "sqlite"))]
        {
            compile_error!("sqlite feature must be enabled!");
            return unreachable!();
        }

        let path = path.as_ref();
        #[cfg(feature = "sqlite")]
        Ok(Self {
            logger,
            // TODO: Allow configuring this. This is SQLite specific.
            positional_param: |_| Cow::Borrowed("?"),
            conn: Connection::Sqlite({
                let mut pool = sqlx::sqlite::SqlitePoolOptions::new();

                // ':memory:' doesn't play nice with the connection pool - https://github.com/launchbadge/sqlx/issues/362#issuecomment-636661146
                if path.to_str() == Some(":memory:") {
                    pool = pool
                        .max_connections(1)
                        .idle_timeout(None)
                        .max_lifetime(None);
                }

                pool.connect_with(
                    sqlx::sqlite::SqliteConnectOptions::new()
                        .create_if_missing(true)
                        .foreign_keys(true)
                        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
                        .filename(path),
                )
                .await
                .unwrap()
            }),
        })
    }

    // TODO: This syntax won't work for multiple DB's
    #[cfg(feature = "sqlite")]
    pub fn sqlite(&self) -> &sqlx::Pool<sqlx::Sqlite> {
        match &self.conn {
            Connection::Sqlite(db) => db,
        }
    }

    // TODO: Using `Cow` style thing for `values`
    pub async fn execute(
        &self,
        sql: impl Into<Cow<'static, str>>,
        // TODO: Allow slice
        values: Vec<SqlValue>,
    ) -> Result<ExecutedQuery, Error> {
        let sql = sql.into();
        (self.logger)(&sql);
        match &self.conn {
            #[cfg(feature = "sqlite")]
            Connection::Sqlite(db) => {
                // TOOD: Should *we* cache this statement?
                // TODO: Should we use `prepare_with`?
                let statement = db.prepare(&sql).await.unwrap();

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

                let rows = query
                    .persistent(false) // TODO: We don't wanna prepare insert mant statements as they can have any number of values
                    .fetch_many(db)
                    .collect::<Vec<
                        Result<
                            Either<sqlx::sqlite::SqliteQueryResult, sqlx::sqlite::SqliteRow>,
                            sqlx::Error,
                        >,
                    >>()
                    .await;
                let mut result: Option<Vec<_>> = None;
                let mut rows_affected = 0;
                let mut last_insert_id = 0;
                for row in rows {
                    let row = row.map_err(|err| Error::Todo(err.to_string()))?;

                    // TODO: Handle more than one result.
                    match row {
                        Either::Left(result) => {
                            rows_affected = result.rows_affected();
                            last_insert_id = result.last_insert_rowid();
                        }
                        Either::Right(row) => {
                            let results = result.get_or_insert_with(Default::default);

                            let mut values = Vec::with_capacity(row.len());
                            for i in 0..row.len() {
                                let y = row.try_get_raw(i).unwrap().to_owned();

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

                return Ok(ExecutedQuery {
                    rows: result,
                    rows_affected,
                    last_insert_id,
                });
            }
        }
    }

    // TODO: Allow providing multiple models.
    pub async fn migrate<T: Table>(&self, _: fn() -> T) -> Result<(), Error> {
        let table = T::def();
        let mut sql = "CREATE TABLE IF NOT EXISTS ".to_string();
        sql.push_str(table.name);
        sql.push_str(" (");
        for (i, col_name) in table.columns.iter().enumerate() {
            let ty = table
                .column_types
                .get(i)
                .expect("Column and column types length mismatch.");
            if i != 0 {
                sql.push_str(", ");
            }
            sql.push_str(col_name);
            sql.push_str(" ");
            // TODO: Do this via a trait cause this is bad.
            sql.push_str(match ty.replace(" ", "").as_str() {
                "u8" | "i32" | "u64" => match i {
                    0 => "INTEGER PRIMARY KEY AUTOINCREMENT", // TODO: Auto incrementing ID should be configured by the user not implied.
                    _ => "INTEGER NOT NULL",
                },
                "bool" => "BOOLEAN NOT NULL",
                "String" => "TEXT NOT NULL",
                "Option<String>" => "TEXT",
                "Vec<u8>" => "BLOB NOT NULL",
                "Option<Vec<u8>>" => "BLOB",
                "uuid::Uuid" | "Uuid" => "BLOB",
                v => todo!("Missing type: {v}"),
            });

            // TODO: Setup `SERIAL` incrementing ID
        }
        sql.push_str(");");

        self.execute(sql, vec![]).await.map(|_| ())
    }

    pub fn select<T: Table>(&self, table: fn() -> T) -> SelectQuery<T::Model> {
        SelectQuery::new(self.clone(), table)
    }

    pub fn insert(&self) -> InsertQuery {
        InsertQuery { db: self.clone() }
    }

    pub fn update<T: Table>(&self, table: fn() -> T) -> UpdateQuery<T::Model> {
        UpdateQuery::new(self.clone(), table)
    }

    pub fn delete<T: Table>(&self, table: fn() -> T) -> DeleteQuery<T::Model> {
        DeleteQuery::new(self.clone(), table)
    }
}
