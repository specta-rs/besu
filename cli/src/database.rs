//! The CLI's view of a database: one chosen from the connection string, plus the two things
//! the CLI wants that the library deliberately does not provide.

use besu::schema::{Change, Ddl, Schema, diff_with};

pub use besu_any::{AnyDatabase, Backend};

/// The CLI's additions to [`AnyDatabase`].
pub trait CliDatabase {
    /// Diff two schemas the way this database would, folding types it cannot tell apart.
    ///
    /// `push` uses the same rule, so `--dry-run` and `generate` report exactly what `push`
    /// would do.
    fn diff(&self, current: &Schema, desired: &Schema) -> Vec<Change>;

    /// Whether this database can change a column without rebuilding the table.
    fn can_alter_column(&self) -> bool;
}

impl CliDatabase for AnyDatabase {
    fn diff(&self, current: &Schema, desired: &Schema) -> Vec<Change> {
        // `push` folds objects a dialect cannot express before diffing; the CLI has to do the
        // same or `--dry-run`/`generate` would report changes that a real push never makes.
        let mut current = current.clone();
        let mut desired = desired.clone();
        match self.backend() {
            Backend::Sqlite => {
                let ddl = besu_sqlite::SQLiteDdl;
                besu::schema::normalise_schema(&ddl, &mut current);
                besu::schema::normalise_schema(&ddl, &mut desired);
                diff_with(&current, &desired, &|ty| ddl.normalise_type(ty))
            }
            Backend::Postgres => {
                let ddl = besu_postgres::PostgresDdl;
                besu::schema::normalise_schema(&ddl, &mut current);
                besu::schema::normalise_schema(&ddl, &mut desired);
                diff_with(&current, &desired, &|ty| ddl.normalise_type(ty))
            }
            Backend::MySql => {
                let ddl = besu_mysql::MySqlDdl;
                besu::schema::normalise_schema(&ddl, &mut current);
                besu::schema::normalise_schema(&ddl, &mut desired);
                diff_with(&current, &desired, &|ty| ddl.normalise_type(ty))
            }
            // `Backend` is `#[non_exhaustive]`: a backend the library learns about before the
            // CLI does compares types exactly, which is conservative but never wrong.
            _ => diff_with(&current, &desired, &|ty| ty.clone()),
        }
    }

    fn can_alter_column(&self) -> bool {
        match self.backend() {
            Backend::Sqlite => besu_sqlite::SQLiteDdl.can_alter_column(),
            Backend::Postgres => besu_postgres::PostgresDdl.can_alter_column(),
            Backend::MySql => besu_mysql::MySqlDdl.can_alter_column(),
            _ => true,
        }
    }
}

/// Render a schema diff as statements for this database.
pub fn apply(
    db: &AnyDatabase,
    changes: &[Change],
    current: &Schema,
    desired: &Schema,
) -> Vec<String> {
    match db.backend() {
        Backend::Sqlite => Ddl::apply_with(&besu_sqlite::SQLiteDdl, changes, current, desired),
        Backend::Postgres => {
            Ddl::apply_with(&besu_postgres::PostgresDdl, changes, current, desired)
        }
        Backend::MySql => Ddl::apply_with(&besu_mysql::MySqlDdl, changes, current, desired),
        _ => Vec::new(),
    }
}
