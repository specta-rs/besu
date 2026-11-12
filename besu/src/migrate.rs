//! Schema generation, introspection and migrations.
//!
//! Everything here is built on one idea: the schema your code declares and the schema a
//! live database has are the *same type*. That makes `push` (diff and apply) and
//! `introspect` (read the database back) two sides of one coin, and it makes drift
//! impossible to miss.
//!
//! ```ignore
//! // Create every table, if they do not exist yet.
//! db.push(&[users.def(), posts.def()]).await?;
//!
//! // Later, after changing a struct: applies only the difference.
//! let result = db.push(&[users.def(), posts.def()]).await?;
//! println!("applied {} statements", result.statements.len());
//! ```

use core::future::Future;

use besu_core::{Dialect, Driver, Sql, TableDef};
use besu_schema::{AppliedMigration, Change, Ddl, MigrationError, Migrator, Schema};

use crate::database::Database;

/// A driver that can describe and change the schema of its database.
///
/// This is a separate trait from [`Driver`] because it is only needed by migrations and
/// the CLI: a library that only runs queries never has to think about it.
pub trait SchemaDriver: Driver {
    /// The DDL dialect for this database.
    type Ddl: Ddl + Default;

    /// Read the database's current schema.
    fn introspect(&self) -> impl Future<Output = Result<Schema, Self::Error>> + Send;

    /// Run a list of schema statements.
    ///
    /// The default runs them one at a time. A driver overrides this when the statements only
    /// make sense together — SQLite's table rebuild toggles `PRAGMA foreign_keys`, which is a
    /// *per-connection* setting, so every statement of a rebuild has to reach the same
    /// connection or the `DROP TABLE` will cascade into child rows.
    fn execute_statements(
        &self,
        statements: &[String],
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async move {
            for statement in statements {
                let sql = Sql::<<Self as Driver>::Dialect>::raw(statement.clone())
                    .finish()
                    .map_err(Self::error)?;
                self.execute(&sql).await?;
            }
            Ok(())
        }
    }
}

/// What a call to [`Database::push`] did.
#[derive(Debug, Clone, Default)]
pub struct PushResult {
    /// The changes that were applied, in order.
    pub changes: Vec<Change>,
    /// The statements that were executed.
    pub statements: Vec<String>,
}

impl PushResult {
    /// Whether the schema already matched and nothing was executed.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Whether any change loses data.
    pub fn is_destructive(&self) -> bool {
        self.changes
            .iter()
            .any(|change| change.safety() == besu_schema::Safety::Destructive)
    }
}

/// Anything that can go wrong while touching the schema.
#[derive(Debug)]
pub enum SchemaError<E> {
    /// The database rejected a statement.
    Driver(E),
    /// A migration file could not be applied.
    Migration(MigrationError<E>),
    /// The requested change would lose data and was not explicitly allowed.
    Destructive(Vec<Change>),
    /// The database could not be introspected.
    Core(besu_core::Error),
}

impl<E: core::fmt::Display> core::fmt::Display for SchemaError<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Driver(err) => write!(f, "{err}"),
            Self::Migration(err) => write!(f, "{err}"),
            Self::Destructive(changes) => {
                writeln!(
                    f,
                    "refusing to apply {} change(s) that would lose data:",
                    changes.len()
                )?;
                for change in changes {
                    writeln!(f, "  - {change}")?;
                }
                write!(f, "call `push_destructive` if you really mean it")
            }
            Self::Core(err) => write!(f, "{err}"),
        }
    }
}

impl<E: core::fmt::Debug + core::fmt::Display> core::error::Error for SchemaError<E> {}

impl<Dr: SchemaDriver> Database<Dr> {
    /// The DDL dialect for this database.
    pub fn ddl() -> Dr::Ddl {
        Dr::Ddl::default()
    }

    /// Build a [`Schema`] from the metadata of the given tables.
    pub fn schema(tables: impl IntoIterator<Item = &'static TableDef>) -> Schema {
        let mut schema = Schema::new();
        for table in tables {
            schema.push_def(table);
        }
        schema.sort();
        schema
    }

    /// Read the database's current schema.
    pub async fn introspect(&self) -> Result<Schema, Dr::Error> {
        self.driver().introspect().await
    }

    /// Apply this database's table prefix to a schema, if it has one.
    ///
    /// Only the object names are rewritten. A view's definition is raw SQL and is not
    /// parsed, so a prefixed view still reads the tables its text names; a schema that uses
    /// both a prefix and views has to write the prefixed table names itself.
    fn with_prefix(&self, schema: &Schema) -> Schema {
        let Some(prefix) = self.prefix() else {
            return schema.clone();
        };

        let mut schema = schema.clone();
        for table in &mut schema.tables {
            table.name = alloc::format!("{prefix}{}", table.name);
            for foreign_key in &mut table.foreign_keys {
                foreign_key.references_table = prefixed_name(prefix, &foreign_key.references_table);
            }
        }
        for view in &mut schema.views {
            view.name = prefixed_name(prefix, &view.name);
        }
        for sequence in &mut schema.sequences {
            sequence.name = alloc::format!("{prefix}{}", sequence.name);
        }
        for policy in &mut schema.policies {
            policy.table = prefixed_name(prefix, &policy.table);
        }
        schema
    }

    /// Create every table in `tables`, without reading the database first.
    ///
    /// A table that already exists is an error from the database, not a no-op: this is the
    /// "create, I know the database is empty" path. Use
    /// [`push`](Database::push) to converge an existing schema.
    pub async fn create_schema(
        &self,
        tables: impl IntoIterator<Item = &'static TableDef>,
    ) -> Result<PushResult, Dr::Error> {
        let schema = self.with_prefix(&Self::schema(tables));
        let statements = <Dr::Ddl as Ddl>::create_schema(&Dr::Ddl::default(), &schema);
        self.driver().execute_statements(&statements).await?;
        Ok(PushResult {
            changes: Vec::new(),
            statements,
        })
    }

    /// Diff the declared tables against the live database and apply the difference.
    ///
    /// Destructive changes are refused. Use [`Database::push_destructive`] to apply them.
    pub async fn push(
        &self,
        tables: impl IntoIterator<Item = &'static TableDef>,
    ) -> Result<PushResult, SchemaError<Dr::Error>> {
        self.push_schema_inner(&Self::schema(tables), false, false)
            .await
    }

    /// Like [`Database::push`], but drops columns and tables when the schema no longer
    /// declares them.
    pub async fn push_destructive(
        &self,
        tables: impl IntoIterator<Item = &'static TableDef>,
    ) -> Result<PushResult, SchemaError<Dr::Error>> {
        self.push_schema_inner(&Self::schema(tables), true, false)
            .await
    }

    /// Diff a schema against the live database and apply the difference.
    pub async fn push_schema(
        &self,
        desired: &Schema,
    ) -> Result<PushResult, SchemaError<Dr::Error>> {
        self.push_schema_inner(desired, false, true).await
    }

    /// Like [`Database::push_schema`], but applies destructive changes too.
    pub async fn push_schema_destructive(
        &self,
        desired: &Schema,
    ) -> Result<PushResult, SchemaError<Dr::Error>> {
        self.push_schema_inner(desired, true, true).await
    }

    /// The JSON snapshot of a schema, ready to hand to `besu push`.
    pub fn schema_snapshot(
        tables: impl IntoIterator<Item = &'static TableDef>,
    ) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&Self::schema(tables))
    }

    async fn push_schema_inner(
        &self,
        desired: &Schema,
        allow_destructive: bool,
        manage_views_and_sequences: bool,
    ) -> Result<PushResult, SchemaError<Dr::Error>> {
        let mut current = self.introspect().await.map_err(SchemaError::Driver)?;
        let mut desired = self.with_prefix(desired);

        // `push(&[TableDef])` cannot express a view or a sequence, so a live one would look
        // undeclared and the plan would propose dropping it — which is destructive, so the
        // whole push would be refused. Carry them across instead: a declaration that cannot
        // mention an object does not get to delete it. A snapshot (`push_schema`) can
        // mention one, so it manages them.
        if !manage_views_and_sequences {
            for view in &current.views {
                if desired.view(&view.name).is_none() {
                    desired.views.push(view.clone());
                }
            }
            for sequence in &current.sequences {
                if desired
                    .find_sequence(sequence.schema.as_deref(), &sequence.name)
                    .is_none()
                {
                    desired.sequences.push(sequence.clone());
                }
            }
        }

        // A shared database holds other tenants' tables. They are not this database's
        // business, so they are dropped from the comparison rather than reported as a
        // `DropTable` — which would be refused, and rightly so. Views, sequences and
        // policies are filtered the same way.
        if let Some(prefix) = self.prefix() {
            current
                .tables
                .retain(|table| table.name.starts_with(prefix));
            current
                .views
                .retain(|view| unqualified(&view.name).starts_with(prefix));
            current
                .sequences
                .retain(|sequence| sequence.name.starts_with(prefix));
            current
                .policies
                .retain(|policy| unqualified(&policy.table).starts_with(prefix));
        }
        let ddl = Dr::Ddl::default();
        let changes = besu_schema::diff_with(&current, &desired, &|ty| ddl.normalise_type(ty));

        if !allow_destructive {
            let destructive: Vec<Change> = changes
                .iter()
                .filter(|change| change.safety() == besu_schema::Safety::Destructive)
                .cloned()
                .collect();
            if !destructive.is_empty() {
                return Err(SchemaError::Destructive(destructive));
            }
        }

        if !Dr::Ddl::default().can_alter_column()
            && changes
                .iter()
                .any(|change| matches!(change, Change::AlterColumn { .. }))
        {
            return Err(SchemaError::Core(besu_core::Error::unsupported(format!(
                "{} cannot change a column in place; write an explicit migration instead",
                <Dr::Ddl as Ddl>::name(&Dr::Ddl::default())
            ))));
        }

        let statements =
            <Dr::Ddl as Ddl>::apply_with(&Dr::Ddl::default(), &changes, &current, &desired);
        self.driver()
            .execute_statements(&statements)
            .await
            .map_err(SchemaError::Driver)?;

        Ok(PushResult {
            changes,
            statements,
        })
    }

    /// Apply a set of migration files.
    ///
    /// The whole run happens inside a transaction, which gives two things at once: the
    /// migrations are atomic, and every statement runs on the *same* connection. That second
    /// part matters for SQLite, where a pool with several connections will happily hand two
    /// statements of a schema change to different connections and fail in confusing ways.
    pub async fn migrate(
        &self,
        migrator: &Migrator,
    ) -> Result<Vec<AppliedMigration>, SchemaError<Dr::Error>>
    where
        Dr: besu_core::TransactionDriver,
        i64: besu_core::Decode<Dr>,
        String: besu_core::Decode<Dr>,
    {
        let mut tx = self.begin().await.map_err(SchemaError::Driver)?;
        let mut executor = &mut tx;
        match migrator.run(&mut executor).await {
            Ok(applied) => {
                tx.commit().await.map_err(SchemaError::Driver)?;
                Ok(applied)
            }
            Err(err) => {
                let _ = tx.rollback().await;
                Err(SchemaError::Migration(err))
            }
        }
    }

    /// Revert applied migrations until the newest one is at most `to`.
    ///
    /// `to = 0` reverts everything. Like [`Database::migrate`], the whole run happens inside a
    /// transaction, which makes it atomic and keeps every statement of a schema change on one
    /// connection. The reverted migrations are returned newest first.
    pub async fn migrate_down(
        &self,
        migrator: &Migrator,
        to: i64,
    ) -> Result<Vec<AppliedMigration>, SchemaError<Dr::Error>>
    where
        Dr: besu_core::TransactionDriver,
        i64: besu_core::Decode<Dr>,
        String: besu_core::Decode<Dr>,
    {
        let mut tx = self.begin().await.map_err(SchemaError::Driver)?;
        let mut executor = &mut tx;
        match migrator.down_to(&mut executor, to).await {
            Ok(reverted) => {
                tx.commit().await.map_err(SchemaError::Driver)?;
                Ok(reverted)
            }
            Err(err) => {
                let _ = tx.rollback().await;
                Err(SchemaError::Migration(err))
            }
        }
    }

    /// Revert only the most recently applied migration.
    ///
    /// Returns the reverted migration, or `None` when nothing has been applied. This is
    /// [`Database::migrate_down`] with a target that is one version back, and it exists
    /// separately because only the journal knows what "one back" is.
    pub async fn migrate_down_one(
        &self,
        migrator: &Migrator,
    ) -> Result<Option<AppliedMigration>, SchemaError<Dr::Error>>
    where
        Dr: besu_core::TransactionDriver,
        i64: besu_core::Decode<Dr>,
        String: besu_core::Decode<Dr>,
    {
        let mut tx = self.begin().await.map_err(SchemaError::Driver)?;
        let mut executor = &mut tx;
        match migrator.down_one(&mut executor).await {
            Ok(reverted) => {
                tx.commit().await.map_err(SchemaError::Driver)?;
                Ok(reverted)
            }
            Err(err) => {
                let _ = tx.rollback().await;
                Err(SchemaError::Migration(err))
            }
        }
    }
}

#[allow(unused)]
fn _keep_dialect<D: Dialect>(_: Option<Sql<D>>) {}

/// Prefix the last segment of a possibly schema-qualified name.
fn prefixed_name(prefix: &str, name: &str) -> String {
    match name.split_once('.') {
        Some((schema, name)) => alloc::format!("{schema}.{prefix}{name}"),
        None => alloc::format!("{prefix}{name}"),
    }
}

/// The name without its schema qualifier.
fn unqualified(name: &str) -> &str {
    match name.split_once('.') {
        Some((_, name)) => name,
        None => name,
    }
}
