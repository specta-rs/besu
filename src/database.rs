use std::{borrow::Cow, path::Path};

use sqlx::{Executor, Pool};

use crate::{queries::Query, schema::Table, SqlValue};

// Sqlx's `Any` connector provides basically no options so we do this. This will also future proof for non-sqlx connectors.
#[derive(Clone)]
pub(crate) enum Connection {
    #[cfg(feature = "sqlite")]
    Sqlite(sqlx::Pool<sqlx::Sqlite>),
}

impl Connection {
    pub(crate) async fn execute(&self, sql: Cow<'static, str>) -> Result<(), ()> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => {
                let _ = db
                    .execute(match &sql {
                        Cow::Borrowed(sql) => sql,
                        Cow::Owned(sql) => sql.as_str(),
                    })
                    .await
                    .map_err(|err| {
                        println!("{err:?}");
                        ()
                    })?; // TODO: Handle response type
                Ok(())
            }
        }
    }
}

pub type LoggerFn = fn(&str) -> ();

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

        #[cfg(feature = "sqlite")]
        Ok(Self {
            logger,
            conn: Connection::Sqlite(
                Pool::connect_with(
                    sqlx::sqlite::SqliteConnectOptions::new()
                        .create_if_missing(true)
                        .foreign_keys(true)
                        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
                        .filename(path),
                )
                .await?,
            ),
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
    ) -> Result<(), ()> {
        let sql = sql.into();
        (self.logger)(&sql);
        self.conn.execute(sql).await
    }

    // TODO: Allow providing multiple models.
    pub async fn migrate<T: Table>(&self, table: fn() -> T) -> Result<(), ()> {
        let table = T::def();
        let mut sql = "CREATE TABLE ".to_string();
        sql.push_str(table.name);
        sql.push_str(" (");
        for (i, (name, ty)) in table.fields.iter().enumerate() {
            if i != 0 {
                sql.push_str(", ");
            }
            sql.push_str(name);
            sql.push_str(" ");
            // TODO: Do this via a trait cause this is bad.
            sql.push_str(match *ty {
                "u8" | "i32" | "u64" => "INTEGER NOT NULL",
                "bool" => "BOOLEAN NOT NULL",
                "String" => "TEXT NOT NULL",
                "Option<String>" => "TEXT",
                v => todo!("Missing type: {v}"),
            });
        }
        sql.push_str(");");

        self.execute(sql, vec![]).await
    }

    // TODO: Rename `select`???
    pub fn query<T: Table>(&self, table: fn() -> T) -> Query<T::Model> {
        Query::new(self.conn.clone(), table)
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
