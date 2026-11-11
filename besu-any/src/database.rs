use besu::schema::{AppliedMigration, Migrator, Schema};
use besu::{Fragment, PushResult, SchemaError, TableDef};

/// Which database an [`AnyDatabase`] is connected to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Backend {
    /// SQLite.
    Sqlite,
    /// PostgreSQL.
    Postgres,
    /// MySQL.
    MySql,
}

impl Backend {
    /// The name the scheme in a connection string uses.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Postgres => "postgres",
            Self::MySql => "mysql",
        }
    }
}

impl core::fmt::Display for Backend {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a statement that returns no rows reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AnyOutput {
    /// The number of rows the statement affected.
    pub rows_affected: u64,
    /// The generated key, where the backend reports one.
    pub last_insert_id: Option<u64>,
}

/// A database chosen at runtime.
///
/// See the [crate documentation](crate) for what this does and does not do.
pub enum AnyDatabase {
    /// A SQLite database.
    Sqlite(besu_sqlite::Database),
    /// A PostgreSQL database.
    Postgres(besu_postgres::Database),
    /// A MySQL database.
    MySql(besu_mysql::Database),
}

impl Clone for AnyDatabase {
    /// Share the underlying connection pool.
    ///
    /// `Database` itself is not `Clone` by design — a query builder borrows it — but the
    /// driver handle inside is a pool, so two `AnyDatabase` values pointing at the same
    /// server is exactly what you want when the backend is chosen at runtime.
    fn clone(&self) -> Self {
        match self {
            Self::Sqlite(db) => Self::Sqlite(besu::Database::new(db.driver().clone())),
            Self::Postgres(db) => Self::Postgres(besu::Database::new(db.driver().clone())),
            Self::MySql(db) => Self::MySql(besu::Database::new(db.driver().clone())),
        }
    }
}

impl core::fmt::Debug for AnyDatabase {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("AnyDatabase").field(&self.backend()).finish()
    }
}

impl AnyDatabase {
    /// Connect to whatever `url` points at, chosen by its scheme.
    ///
    /// `sqlite:` and `sqlite://` open a file (creating it if it does not exist), `postgres://`
    /// and `postgresql://` a PostgreSQL server, and `mysql://` a MySQL server.
    pub async fn connect(url: &str) -> Result<Self, sqlx::Error> {
        let scheme = url
            .split_once(':')
            .map(|(scheme, _)| scheme)
            .unwrap_or_default()
            .to_ascii_lowercase();

        Ok(match scheme.as_str() {
            "sqlite" => Self::Sqlite(besu::Database::new(besu_sqlite::Sqlx::connect(url).await?)),
            "postgres" | "postgresql" => Self::Postgres(besu::Database::new(
                besu_postgres::Sqlx::connect(url).await?,
            )),
            "mysql" => Self::MySql(besu::Database::new(besu_mysql::Sqlx::connect(url).await?)),
            other => {
                return Err(sqlx::Error::Configuration(alloc_message(other).into()));
            }
        })
    }

    /// Which database this is.
    pub fn backend(&self) -> Backend {
        match self {
            Self::Sqlite(_) => Backend::Sqlite,
            Self::Postgres(_) => Backend::Postgres,
            Self::MySql(_) => Backend::MySql,
        }
    }

    /// Read the database's current schema.
    pub async fn introspect(&self) -> Result<Schema, sqlx::Error> {
        Ok(match self {
            Self::Sqlite(db) => db.introspect().await?,
            Self::Postgres(db) => db.introspect().await?,
            Self::MySql(db) => db.introspect().await?,
        })
    }

    /// Apply a schema, refusing destructive changes unless `destructive` is set.
    pub async fn push_schema(
        &self,
        schema: &Schema,
        destructive: bool,
    ) -> Result<PushResult, SchemaError<sqlx::Error>> {
        match self {
            Self::Sqlite(db) => push(db, schema, destructive).await,
            Self::Postgres(db) => push(db, schema, destructive).await,
            Self::MySql(db) => push(db, schema, destructive).await,
        }
    }

    /// Apply the tables your code declares, refusing destructive changes unless `destructive`
    /// is set.
    pub async fn push_tables(
        &self,
        tables: impl IntoIterator<Item = &'static TableDef>,
        destructive: bool,
    ) -> Result<PushResult, SchemaError<sqlx::Error>> {
        let schema = Schema::from_defs(&tables.into_iter().collect::<Vec<_>>());
        self.push_schema(&schema, destructive).await
    }

    /// Apply migration files.
    pub async fn migrate(
        &self,
        migrator: &Migrator,
    ) -> Result<Vec<AppliedMigration>, SchemaError<sqlx::Error>> {
        match self {
            Self::Sqlite(db) => db.migrate(migrator).await,
            Self::Postgres(db) => db.migrate(migrator).await,
            Self::MySql(db) => db.migrate(migrator).await,
        }
    }

    /// Revert migrations down to `to`, where `0` reverts everything.
    pub async fn migrate_down(
        &self,
        migrator: &Migrator,
        to: i64,
    ) -> Result<Vec<AppliedMigration>, SchemaError<sqlx::Error>> {
        match self {
            Self::Sqlite(db) => db.migrate_down(migrator, to).await,
            Self::Postgres(db) => db.migrate_down(migrator, to).await,
            Self::MySql(db) => db.migrate_down(migrator, to).await,
        }
    }

    /// Revert the most recently applied migration.
    pub async fn migrate_down_one(
        &self,
        migrator: &Migrator,
    ) -> Result<Option<AppliedMigration>, SchemaError<sqlx::Error>> {
        match self {
            Self::Sqlite(db) => db.migrate_down_one(migrator).await,
            Self::Postgres(db) => db.migrate_down_one(migrator).await,
            Self::MySql(db) => db.migrate_down_one(migrator).await,
        }
    }

    /// Run a statement that returns no rows.
    pub async fn execute(&self, fragment: Fragment) -> Result<AnyOutput, sqlx::Error> {
        Ok(match self {
            Self::Sqlite(db) => {
                let out = db.execute(fragment).await?;
                AnyOutput {
                    rows_affected: out.rows_affected(),
                    last_insert_id: u64::try_from(out.last_insert_rowid()).ok(),
                }
            }
            Self::Postgres(db) => {
                let out = db.execute(fragment).await?;
                AnyOutput {
                    rows_affected: out.rows_affected(),
                    last_insert_id: None,
                }
            }
            Self::MySql(db) => {
                let out = db.execute(fragment).await?;
                AnyOutput {
                    rows_affected: out.rows_affected(),
                    last_insert_id: Some(out.last_insert_id()),
                }
            }
        })
    }
}

async fn push<Dr: besu::SchemaDriver>(
    db: &besu::Database<Dr>,
    schema: &Schema,
    destructive: bool,
) -> Result<PushResult, SchemaError<Dr::Error>> {
    if destructive {
        db.push_schema_destructive(schema).await
    } else {
        db.push_schema(schema).await
    }
}

fn alloc_message(scheme: &str) -> String {
    format!("unsupported database scheme `{scheme}`; expected sqlite, postgres or mysql")
}
