use besu::schema::{AppliedMigration, Migrator, Schema};
use besu::{PushResult, SchemaError};

/// A database of any supported kind.
///
/// The CLI is the one place where the concrete backend is only known at runtime, so it
/// dispatches over an enum. Application code never needs this: it picks a driver at compile
/// time and gets the full typed API.
pub enum AnyDatabase {
    /// A SQLite database.
    Sqlite(besu_sqlite::Database),
    /// A PostgreSQL database.
    Postgres(besu_postgres::Database),
    /// A MySQL database.
    MySql(besu_mysql::Database),
}

impl AnyDatabase {
    /// Connect to whatever `url` points at, chosen by its scheme.
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let scheme = url
            .split_once(':')
            .map(|(scheme, _)| scheme)
            .unwrap_or_default()
            .to_ascii_lowercase();

        Ok(match scheme.as_str() {
            "sqlite" => Self::Sqlite(besu_sqlite::Database::new(
                besu_sqlite::Sqlx::connect(url).await?,
            )),
            "postgres" | "postgresql" => Self::Postgres(besu_postgres::Database::new(
                besu_postgres::Sqlx::connect(url).await?,
            )),
            "mysql" => Self::MySql(besu_mysql::Database::new(
                besu_mysql::Sqlx::connect(url).await?,
            )),
            other => anyhow::bail!(
                "unsupported database scheme `{other}`; expected sqlite, postgres or mysql"
            ),
        })
    }

    /// The name of the database kind, for diagnostics.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Sqlite(_) => "sqlite",
            Self::Postgres(_) => "postgres",
            Self::MySql(_) => "mysql",
        }
    }

    /// Read the database's current schema.
    pub async fn introspect(&self) -> anyhow::Result<Schema> {
        Ok(match self {
            Self::Sqlite(db) => db.introspect().await?,
            Self::Postgres(db) => db.introspect().await?,
            Self::MySql(db) => db.introspect().await?,
        })
    }

    /// Apply `schema`, optionally allowing destructive changes.
    pub async fn push(&self, schema: &Schema, destructive: bool) -> anyhow::Result<PushResult> {
        let result = match self {
            Self::Sqlite(db) => push(db, schema, destructive).await,
            Self::Postgres(db) => push(db, schema, destructive).await,
            Self::MySql(db) => push(db, schema, destructive).await,
        };
        result.map_err(into_anyhow)
    }

    /// Apply migration files.
    pub async fn migrate(&self, migrator: &Migrator) -> anyhow::Result<Vec<AppliedMigration>> {
        let result = match self {
            Self::Sqlite(db) => db.migrate(migrator).await,
            Self::Postgres(db) => db.migrate(migrator).await,
            Self::MySql(db) => db.migrate(migrator).await,
        };
        result.map_err(into_anyhow)
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

fn into_anyhow(err: SchemaError<sqlx::Error>) -> anyhow::Error {
    anyhow::anyhow!("{err}")
}
