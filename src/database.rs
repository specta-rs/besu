use std::{borrow::Cow, path::Path};

use futures::stream::StreamExt;
use sqlx::{Either, Error, Executor, Row, ValueRef};

use crate::{queries::Query, schema::Table, SqlValue};

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
    conn: Connection,
}

impl Database {
    pub async fn new(path: impl AsRef<Path>) -> Result<Self, sqlx::Error> {
        // Self::new_with_logger(path, |_| ()).await
        Self::new_with_logger(path, |sql| println!("EXECUTE: {sql}")).await // TODO: Remove this line
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
        values: Vec<SqlValue>,
    ) -> Result<Option<Vec<Vec<SqlValue>>>, ()> {
        let sql = sql.into();
        (self.logger)(&sql);
        match &self.conn {
            #[cfg(feature = "sqlite")]
            Connection::Sqlite(db) => {
                let rows = db
                    .fetch_many(match &sql {
                        Cow::Borrowed(sql) => sql,
                        Cow::Owned(sql) => sql.as_str(),
                    })
                    .collect::<Vec<
                        Result<
                            Either<sqlx::sqlite::SqliteQueryResult, sqlx::sqlite::SqliteRow>,
                            Error,
                        >,
                    >>()
                    .await;
                let mut result: Option<Vec<_>> = None;
                for row in rows {
                    let row = row.map_err(|err| {
                        println!("{err:?}");
                        ()
                    })?;

                    // TODO: Handle more than one result.
                    match row {
                        Either::Left(_) => {}
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

                return Ok(result);
            }
        }
    }

    // TODO: Allow providing multiple models.
    pub async fn migrate<T: Table>(&self, _: fn() -> T) -> Result<(), ()> {
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
            sql.push_str(match *ty {
                "u8" | "i32" | "u64" => "INTEGER NOT NULL",
                "bool" => "BOOLEAN NOT NULL",
                "String" => "TEXT NOT NULL",
                "Option<String>" => "TEXT",
                v => todo!("Missing type: {v}"),
            });

            // TODO: Setup `SERIAL` incrementing ID
        }
        sql.push_str(");");

        self.execute(sql, vec![]).await.map(|_| ())
    }

    pub fn select<T: Table>(&self, table: fn() -> T) -> Query<T::Model> {
        Query::new(self.clone(), table)
    }

    // pub fn insert(&self) -> InsertQuery {
    //     InsertQuery {}
    // }

    // // TODO: Generics
    // pub fn update(&self, model: ()) -> UpdateQuery<()> {
    //     UpdateQuery {
    //         phantom: PhantomData,
    //     }
    // }

    // pub fn count(&self)
}
