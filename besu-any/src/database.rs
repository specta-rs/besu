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
    /// server is exactly what you want when the backend is chosen at runtime. The table
    /// prefix is part of how the `Database` renders its SQL, so it is carried across too;
    /// dropping it would silently make the clone address unprefixed tables.
    fn clone(&self) -> Self {
        match self {
            Self::Sqlite(db) => Self::Sqlite(clone_database(db)),
            Self::Postgres(db) => Self::Postgres(clone_database(db)),
            Self::MySql(db) => Self::MySql(clone_database(db)),
        }
    }
}

/// Clone a `Database`, keeping the table prefix it was configured with.
fn clone_database<Dr: besu::Driver>(db: &besu::Database<Dr>) -> besu::Database<Dr> {
    match db.prefix() {
        Some(prefix) => besu::Database::prefixed(db.driver().clone(), prefix),
        None => besu::Database::new(db.driver().clone()),
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
        Ok(match backend_for_url(url)? {
            Backend::Sqlite => {
                Self::Sqlite(besu::Database::new(besu_sqlite::Sqlx::connect(url).await?))
            }
            Backend::Postgres => Self::Postgres(besu::Database::new(
                besu_postgres::Sqlx::connect(url).await?,
            )),
            Backend::MySql => {
                Self::MySql(besu::Database::new(besu_mysql::Sqlx::connect(url).await?))
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
    ///
    /// This is the table-definition path, not the snapshot path: a table definition cannot
    /// mention a view or a sequence, so live ones are carried across rather than reported as
    /// undeclared and dropped. Using [`AnyDatabase::push_schema`] here instead would make
    /// `push_tables` refuse a plan over every view it never had a way to declare.
    pub async fn push_tables(
        &self,
        tables: impl IntoIterator<Item = &'static TableDef>,
        destructive: bool,
    ) -> Result<PushResult, SchemaError<sqlx::Error>> {
        let tables: Vec<&'static TableDef> = tables.into_iter().collect();
        match self {
            Self::Sqlite(db) => push_tables(db, &tables, destructive).await,
            Self::Postgres(db) => push_tables(db, &tables, destructive).await,
            Self::MySql(db) => push_tables(db, &tables, destructive).await,
        }
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
        // SQLite's `last_insert_rowid` is per connection and survives unrelated statements,
        // and MySQL reports `0` for a statement that generated no key. Asking what the
        // statement is makes the three backends agree: a generated key is reported only for
        // the statements that can generate one.
        let generated_key = wants_last_insert_id(fragment.sql());
        Ok(match self {
            Self::Sqlite(db) => {
                let out = db.execute(fragment).await?;
                AnyOutput {
                    rows_affected: out.rows_affected(),
                    last_insert_id: generated_key
                        .then(|| u64::try_from(out.last_insert_rowid()).ok())
                        .flatten(),
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
                    // Zero is MySQL's "no auto-increment value was generated", not a key.
                    last_insert_id: generated_key
                        .then(|| out.last_insert_id())
                        .filter(|id| *id != 0),
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

/// Apply declared tables through the table-definition path, which keeps undeclared views and
/// sequences instead of treating them as removed.
async fn push_tables<Dr: besu::SchemaDriver>(
    db: &besu::Database<Dr>,
    tables: &[&'static TableDef],
    destructive: bool,
) -> Result<PushResult, SchemaError<Dr::Error>> {
    if destructive {
        db.push_destructive(tables.iter().copied()).await
    } else {
        db.push(tables.iter().copied()).await
    }
}

/// The backend a connection string names, chosen by its scheme.
///
/// The scheme is matched case-insensitively: an environment variable is whatever the
/// deployment put there, and `POSTGRES://` names the same backend as `postgres://`.
fn backend_for_url(url: &str) -> Result<Backend, sqlx::Error> {
    let scheme = url
        .split_once(':')
        .map(|(scheme, _)| scheme)
        .unwrap_or_default()
        .to_ascii_lowercase();

    match scheme.as_str() {
        "sqlite" => Ok(Backend::Sqlite),
        "postgres" | "postgresql" => Ok(Backend::Postgres),
        "mysql" => Ok(Backend::MySql),
        other => Err(sqlx::Error::Configuration(alloc_message(other).into())),
    }
}

/// Whether a statement can report a generated key.
///
/// Only `INSERT` (including SQLite's `INSERT OR ...` spellings) and `REPLACE` create a row
/// with a generated key. A leading comment is skipped so a hand-written fragment annotated
/// above the verb is classified the same way. A statement this cannot see a verb for — a
/// `WITH ... INSERT`, for example — reports no key rather than whatever the connection did
/// last.
fn wants_last_insert_id(sql: &str) -> bool {
    let mut rest = sql.trim_start();
    loop {
        if let Some(after) = rest.strip_prefix("--") {
            rest = after
                .split_once('\n')
                .map_or("", |(_, rest)| rest)
                .trim_start();
            continue;
        }
        if let Some(after) = rest.strip_prefix("/*") {
            rest = after
                .split_once("*/")
                .map_or("", |(_, rest)| rest)
                .trim_start();
            continue;
        }
        break;
    }
    let verb = rest
        .split(|ch: char| ch.is_whitespace() || matches!(ch, '(' | ';'))
        .next()
        .unwrap_or_default();
    verb.eq_ignore_ascii_case("insert") || verb.eq_ignore_ascii_case("replace")
}

fn alloc_message(scheme: &str) -> String {
    format!("unsupported database scheme `{scheme}`; expected sqlite, postgres or mysql")
}

#[cfg(test)]
mod tests {
    use super::{Backend, backend_for_url, clone_database};

    /// A driver with no connection, so cloning can be exercised without a server or a
    /// runtime. Nothing in these tests runs a statement.
    #[derive(Clone, Debug)]
    struct FakeDriver;

    impl besu::Driver for FakeDriver {
        type Dialect = besu_sqlite::SQLite;
        type Error = std::io::Error;
        type Row = ();
        type Output = ();

        fn error(error: besu::Error) -> Self::Error {
            std::io::Error::other(error.to_string())
        }

        fn row_len(_row: &Self::Row) -> usize {
            0
        }

        fn is_null(_row: &Self::Row, _index: usize) -> bool {
            true
        }

        fn query(
            &self,
            _sql: &besu::Sql<Self::Dialect>,
        ) -> impl core::future::Future<Output = Result<Vec<Self::Row>, Self::Error>> + Send
        {
            core::future::ready(Ok(Vec::new()))
        }

        fn execute(
            &self,
            _sql: &besu::Sql<Self::Dialect>,
        ) -> impl core::future::Future<Output = Result<Self::Output, Self::Error>> + Send {
            core::future::ready(Ok(()))
        }
    }

    #[test]
    fn cloning_keeps_the_table_prefix() {
        // The prefix is part of every statement the `Database` renders. A clone that dropped
        // it would silently address the unprefixed tables of another tenant.
        let prefixed = besu::Database::<FakeDriver>::prefixed(FakeDriver, "tenant_");
        assert_eq!(clone_database(&prefixed).prefix(), Some("tenant_"));

        let plain = besu::Database::<FakeDriver>::new(FakeDriver);
        assert_eq!(clone_database(&plain).prefix(), None);
    }

    #[test]
    fn a_url_is_dispatched_by_its_scheme() {
        let backend = |url: &str| backend_for_url(url).expect("the scheme must be supported");
        assert_eq!(backend("sqlite:app.db"), Backend::Sqlite);
        assert_eq!(backend("sqlite://app.db"), Backend::Sqlite);
        assert_eq!(backend("postgres://user:pass@host/db"), Backend::Postgres);
        assert_eq!(backend("postgresql://user:pass@host/db"), Backend::Postgres);
        assert_eq!(backend("mysql://user:pass@host/db"), Backend::MySql);
        // The scheme is case-insensitive.
        assert_eq!(backend("POSTGRES://user:pass@host/db"), Backend::Postgres);
    }

    #[test]
    fn an_unsupported_scheme_names_itself() {
        let message = backend_for_url("oracle://host/db")
            .expect_err("oracle is not supported")
            .to_string();
        assert!(message.contains("oracle"), "{message}");
        assert!(message.contains("sqlite"), "{message}");
        assert!(message.contains("postgres"), "{message}");
        assert!(message.contains("mysql"), "{message}");

        // A bare file path has no scheme at all, which is equally unsupported.
        assert!(backend_for_url("app.db").is_err());
    }

    #[test]
    fn only_an_insert_reports_a_generated_key() {
        use super::wants_last_insert_id;

        assert!(wants_last_insert_id("INSERT INTO users (name) VALUES (?)"));
        assert!(wants_last_insert_id("  insert or replace into users ..."));
        assert!(wants_last_insert_id("REPLACE INTO users ..."));
        // A leading comment is not the verb.
        assert!(wants_last_insert_id(
            "-- seed the table\nINSERT INTO users ..."
        ));
        assert!(wants_last_insert_id("/* seed */ INSERT INTO users ..."));

        // SQLite's `last_insert_rowid` and MySQL's `last_insert_id` are connection state,
        // so reporting them for one of these would attribute another statement's key.
        assert!(!wants_last_insert_id("UPDATE users SET name = ?"));
        assert!(!wants_last_insert_id("DELETE FROM users"));
        assert!(!wants_last_insert_id("CREATE TABLE users (id BIGINT)"));
        assert!(!wants_last_insert_id(
            "WITH new AS (SELECT 1) SELECT * FROM new"
        ));
        assert!(!wants_last_insert_id(""));
    }
}
