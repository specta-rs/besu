use std::path::Path;

use sqlx::{Executor, Pool};

use crate::{queries::Query, schema::Table};

// Sqlx's `Any` connector provides basically no options so we do this. This will also future proof for non-sqlx connectors.
#[derive(Clone)]
pub(crate) enum Connection {
    #[cfg(feature = "sqlite")]
    Sqlite(sqlx::Pool<sqlx::Sqlite>),
}

impl Connection {
    pub(crate) async fn execute(&self, sql: String) -> Result<(), ()> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => {
                let _ = db.execute(sql.as_str()).await.map_err(|err| {
                    println!("{err:?}");
                    ()
                })?; // TODO: Handle response type
                Ok(())
            }
        }
    }
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

    pub fn query<M>(&self, table: Table<M>) -> Query<M> {
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
