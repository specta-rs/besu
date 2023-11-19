use std::{marker::PhantomData, path::Path};

use sqlx::{ConnectOptions, Pool};

use crate::{
    queries::{InsertQuery, Query, UpdateQuery},
    schema::Table,
};

// Sqlx's `Any` connector provides basically no options so we do this. This will also future proof for non-sqlx connectors.
#[derive(Clone)]
pub(crate) enum Connection {
    #[cfg(feature = "sqlite")]
    Sqlite(sqlx::Pool<sqlx::Sqlite>),
}

pub struct Database {
    conn: Connection,
}

impl Database {
    pub async fn new(path: impl AsRef<Path>) -> Result<Self, sqlx::Error> {
        #[cfg(not(feature = "sqlite"))]
        {
            compile_error!("sqlite feature must be enabled!");
            return unreachable!();
        }

        #[cfg(feature = "sqlite")]
        Ok(Self {
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

    pub fn query<T: Table>(&self, table: T) -> Query<T> {
        Query::new()
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
