use core::fmt;

use besu_core::{Decode, ErrorOf, Executor, Sql};

/// One migration: a version, a human name and the SQL to run, forwards and back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migration {
    /// Monotonically increasing version. Migrations are applied in this order.
    pub version: i64,
    /// A short human-readable name, used for the filename and the journal.
    pub name: String,
    /// One or more SQL statements that apply the change.
    pub statements: Vec<String>,
    /// One or more SQL statements that undo it, from a `.down.sql` file.
    ///
    /// `None` means the migration cannot be reverted, which is the default.
    pub down: Option<Vec<String>>,
}

impl Migration {
    /// Build a migration from a single SQL blob, splitting statements on `;`.
    ///
    /// The result has no down migration; use [`Migration::from_sql_pair`] when there is one.
    pub fn from_sql(version: i64, name: impl Into<String>, sql: impl Into<String>) -> Self {
        Self::from_sql_pair(version, name, sql, None)
    }

    /// Build a migration and its down migration from two SQL blobs.
    ///
    /// Both are split on `;` the same way. `down` is `None` when the migration cannot be
    /// reverted.
    pub fn from_sql_pair(
        version: i64,
        name: impl Into<String>,
        up: impl Into<String>,
        down: Option<String>,
    ) -> Self {
        Self {
            version,
            name: name.into(),
            statements: split_statements(&up.into()),
            down: down.map(|sql| split_statements(&sql)),
        }
    }

    /// A stable checksum of the migration's contents.
    ///
    /// Changing an already-applied migration is almost always a mistake, so the runner
    /// refuses to continue when the checksum no longer matches.
    pub fn checksum(&self) -> String {
        checksum_of(&self.name, "", &self.statements)
    }

    /// A stable checksum of the down statements, or `None` when there are none.
    ///
    /// It is recorded in the journal when the migration is applied, so an edited down file
    /// is caught the same way an edited up file is.
    pub fn down_checksum(&self) -> Option<String> {
        self.down
            .as_ref()
            .map(|statements| checksum_of(&self.name, "down", statements))
    }
}

/// FNV-1a over a migration's name, its direction and its statements.
///
/// The up direction contributes no bytes, so checksums recorded before down migrations
/// existed still match.
fn checksum_of(name: &str, direction: &str, statements: &[String]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    feed(name.as_bytes());
    feed(&[0]);
    feed(direction.as_bytes());
    feed(statements.join("\n").as_bytes());
    format!("{hash:016x}")
}

/// A migration that has been applied to a database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedMigration {
    /// The version that was applied.
    pub version: i64,
    /// The name recorded at apply time.
    pub name: String,
    /// The checksum recorded at apply time.
    pub checksum: String,
    /// The down checksum recorded at apply time, when the migration had a down file.
    ///
    /// It is `None` for a migration that has no down file, and for one applied before the
    /// journal recorded down checksums.
    pub down_checksum: Option<String>,
}

/// Anything that can go wrong while migrating.
#[derive(Debug)]
#[non_exhaustive]
pub enum MigrationError<E> {
    /// The database rejected a statement.
    Driver(E),
    /// The database refused to report its migration history.
    Core(besu_core::Error),
    /// A previously applied migration has been edited since it ran.
    ChecksumMismatch {
        /// The version that was edited.
        version: i64,
        /// What the journal says.
        expected: String,
        /// What the file says now.
        actual: String,
    },
    /// Two migrations share a version.
    DuplicateVersion(i64),
    /// The newest applied migration cannot be reverted, because it has no down file.
    MissingDownFile {
        /// The version that cannot be reverted.
        version: i64,
    },
}

impl<E: fmt::Display> fmt::Display for MigrationError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Driver(err) => write!(f, "{err}"),
            Self::Core(err) => write!(f, "{err}"),
            Self::ChecksumMismatch {
                version,
                expected,
                actual,
            } => write!(
                f,
                "migration {version} has been modified since it was applied \
                 (recorded checksum {expected}, now {actual}); migrations are append-only"
            ),
            Self::DuplicateVersion(version) => write!(f, "two migrations share version {version}"),
            Self::MissingDownFile { version } => write!(
                f,
                "migration {version} has no down file, so it cannot be reverted; \
                 add a `.down.sql` file beside it"
            ),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> core::error::Error for MigrationError<E> {}

/// Applies migration files to a database.
///
/// The runner is generic over an [`Executor`], so the same code migrates a SQLite file, a
/// Postgres pool or an in-flight transaction. Pass a transaction to get all-or-nothing
/// semantics; pass a pool and each migration commits as it goes.
#[derive(Debug, Clone, Default)]
pub struct Migrator {
    migrations: Vec<Migration>,
}

impl Migrator {
    /// An empty migrator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a migration.
    pub fn push(&mut self, migration: Migration) {
        self.migrations.push(migration);
    }

    /// Add many migrations.
    pub fn extend(&mut self, migrations: impl IntoIterator<Item = Migration>) {
        self.migrations.extend(migrations);
    }

    /// The migrations, sorted by version.
    pub fn migrations(&self) -> Vec<&Migration> {
        let mut migrations: Vec<&Migration> = self.migrations.iter().collect();
        migrations.sort_by_key(|m| m.version);
        migrations
    }

    /// Whether there are no migrations at all.
    pub fn is_empty(&self) -> bool {
        self.migrations.is_empty()
    }

    /// How many migrations there are.
    pub fn len(&self) -> usize {
        self.migrations.len()
    }

    /// Read `*.sql` files from a directory.
    ///
    /// Files are named `<version>_<name>.sql`, for example `0007_add_email_index.sql`. A file
    /// named `<version>_<name>.down.sql` is the down migration for the same version, and is
    /// paired with the up file whatever its name. Either half may be missing on its own, but a
    /// down file without an up file is an error.
    #[cfg(feature = "fs")]
    pub fn from_dir(dir: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let mut ups: Vec<(i64, String, String)> = Vec::new();
        let mut downs: Vec<(i64, String, String)> = Vec::new();

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("sql") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let (stem, is_down) = match stem.strip_suffix(".down") {
                Some(stem) => (stem, true),
                None => (stem, false),
            };
            let (version, name) = match stem.split_once('_') {
                Some((version, name)) => (version, name),
                None => (stem, ""),
            };
            let Ok(version) = version.parse::<i64>() else {
                continue;
            };
            let sql = std::fs::read_to_string(&path)?;
            if is_down {
                downs.push((version, path.display().to_string(), sql));
            } else {
                ups.push((version, name.replace('_', " "), sql));
            }
        }

        // A down file is paired by version, which is the identity the journal records, and
        // not by name: a typo in a name should not silently lose the down migration.
        let mut paired = vec![false; downs.len()];
        let mut migrations = Vec::with_capacity(ups.len());
        for (version, name, sql) in ups {
            let mut down = None;
            for (index, (down_version, _, down_sql)) in downs.iter().enumerate() {
                if !paired[index] && *down_version == version {
                    paired[index] = true;
                    down = Some(down_sql.clone());
                    break;
                }
            }
            migrations.push(Migration::from_sql_pair(version, name, sql, down));
        }

        for (index, (_, path, _)) in downs.iter().enumerate() {
            if !paired[index] {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{path} has no matching up migration"),
                ));
            }
        }

        migrations.sort_by_key(|m| m.version);
        Ok(Self { migrations })
    }

    /// Apply every migration that has not run yet.
    ///
    /// Returns the newly applied migrations, in order.
    pub async fn run<E: Executor>(
        &self,
        exec: &mut E,
    ) -> Result<Vec<AppliedMigration>, MigrationError<ErrorOf<E>>>
    where
        i64: Decode<E::Driver>,
        String: Decode<E::Driver>,
    {
        // 1. Make sure the journal exists and is up to date.
        let applied = read_journal(exec).await?;

        // 2. Validate: no duplicates, and nothing that already ran was edited.
        let mut sorted = self.migrations.clone();
        sorted.sort_by_key(|m| m.version);
        for pair in sorted.windows(2) {
            if pair[0].version == pair[1].version {
                return Err(MigrationError::DuplicateVersion(pair[0].version));
            }
        }
        for migration in &sorted {
            if let Some(record) = applied.iter().find(|a| a.version == migration.version) {
                verify_checksums(record, migration)?;
            }
        }

        // 3. Apply the missing ones.
        let mut newly_applied = Vec::new();
        for migration in &sorted {
            if applied.iter().any(|a| a.version == migration.version) {
                continue;
            }

            for statement in &migration.statements {
                let sql = Sql::<besu_core::executor::DialectOf<E>>::raw(statement.clone());
                match exec.execute(&sql).await {
                    Ok(_) => {}
                    Err(err) => return Err(MigrationError::Driver(err)),
                }
            }

            let mut insert = Sql::<besu_core::executor::DialectOf<E>>::raw(
                "INSERT INTO _besu_migrations (version, name, checksum, down_checksum) VALUES (",
            );
            insert.bind(migration.version);
            insert.push_raw(", ");
            insert.bind(migration.name.clone());
            insert.push_raw(", ");
            insert.bind(migration.checksum());
            insert.push_raw(", ");
            insert.bind(migration.down_checksum());
            insert.push_raw(")");
            exec.execute(&insert)
                .await
                .map_err(MigrationError::Driver)?;

            newly_applied.push(AppliedMigration {
                version: migration.version,
                name: migration.name.clone(),
                checksum: migration.checksum(),
                down_checksum: migration.down_checksum(),
            });
        }

        Ok(newly_applied)
    }

    /// Revert the single most recently applied migration.
    ///
    /// Returns the reverted migration, or `None` when the journal is empty. The newest
    /// migration must have a down file: reverting is explicit, and a migration that cannot be
    /// undone stops the run rather than being skipped.
    ///
    /// Pass a transaction to get all-or-nothing semantics; pass a pool and each statement
    /// commits as it goes, exactly like [`Migrator::run`].
    pub async fn down_one<E: Executor>(
        &self,
        exec: &mut E,
    ) -> Result<Option<AppliedMigration>, MigrationError<ErrorOf<E>>>
    where
        i64: Decode<E::Driver>,
        String: Decode<E::Driver>,
    {
        let applied = read_journal(exec).await?;
        match applied.into_iter().next_back() {
            Some(newest) => Ok(Some(self.revert(exec, newest).await?)),
            None => Ok(None),
        }
    }

    /// Revert applied migrations until the highest applied version is at most `to`.
    ///
    /// Returns the reverted migrations, newest first. `to = 0` reverts everything.
    pub async fn down_to<E: Executor>(
        &self,
        exec: &mut E,
        to: i64,
    ) -> Result<Vec<AppliedMigration>, MigrationError<ErrorOf<E>>>
    where
        i64: Decode<E::Driver>,
        String: Decode<E::Driver>,
    {
        let mut reverted = Vec::new();
        loop {
            let applied = read_journal(exec).await?;
            let Some(newest) = applied.into_iter().next_back() else {
                break;
            };
            if newest.version <= to {
                break;
            }
            reverted.push(self.revert(exec, newest).await?);
        }
        Ok(reverted)
    }

    /// Undo one journal row: run its down statements in reverse, then forget it.
    async fn revert<E: Executor>(
        &self,
        exec: &mut E,
        record: AppliedMigration,
    ) -> Result<AppliedMigration, MigrationError<ErrorOf<E>>>
    where
        i64: Decode<E::Driver>,
        String: Decode<E::Driver>,
    {
        let migration = self
            .migrations
            .iter()
            .find(|m| m.version == record.version)
            .ok_or(MigrationError::MissingDownFile {
                version: record.version,
            })?;
        let Some(statements) = &migration.down else {
            return Err(MigrationError::MissingDownFile {
                version: record.version,
            });
        };

        verify_checksums::<ErrorOf<E>>(&record, migration)?;

        // Down statements run from the bottom of the file up, mirroring the order the up
        // file applies them in.
        for statement in statements.iter().rev() {
            let sql = Sql::<besu_core::executor::DialectOf<E>>::raw(statement.clone());
            exec.execute(&sql).await.map_err(MigrationError::Driver)?;
        }

        let mut delete = Sql::<besu_core::executor::DialectOf<E>>::raw(
            "DELETE FROM _besu_migrations WHERE version = ",
        );
        delete.bind(record.version);
        exec.execute(&delete)
            .await
            .map_err(MigrationError::Driver)?;

        Ok(record)
    }
}

/// Create the journal if it is missing, then read what has already run, oldest first.
async fn read_journal<E: Executor>(
    exec: &mut E,
) -> Result<Vec<AppliedMigration>, MigrationError<ErrorOf<E>>>
where
    i64: Decode<E::Driver>,
    String: Decode<E::Driver>,
{
    // This statement is portable across the three supported databases on purpose: the journal
    // must be readable before any dialect-specific machinery is involved.
    let create = Sql::raw(
        "CREATE TABLE IF NOT EXISTS _besu_migrations (\
         version BIGINT PRIMARY KEY, \
         name VARCHAR(255) NOT NULL, \
         checksum VARCHAR(64) NOT NULL, \
         down_checksum VARCHAR(64))",
    );
    exec.execute(&create)
        .await
        .map_err(MigrationError::Driver)?;

    // A journal written before down migrations existed has three columns.
    //
    // The question has to be asked with a statement that *succeeds* either way: PostgreSQL
    // aborts the enclosing transaction when a statement fails, and the migration runner works
    // inside one, so probing with a deliberately broken `SELECT` would poison the transaction
    // on exactly the upgrade it is meant to perform.
    if !journal_has_down_checksum(exec)
        .await
        .map_err(MigrationError::Driver)?
    {
        let alter = Sql::raw("ALTER TABLE _besu_migrations ADD COLUMN down_checksum VARCHAR(64)");
        exec.execute(&alter).await.map_err(MigrationError::Driver)?;
    }

    let select = Sql::raw(
        "SELECT version, name, checksum, down_checksum FROM _besu_migrations ORDER BY version",
    );
    let rows = exec.query(&select).await.map_err(MigrationError::Driver)?;

    let mut applied: Vec<AppliedMigration> = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut offset = 0;
        let version = i64::decode(row, &mut offset).map_err(MigrationError::Driver)?;
        let name = String::decode(row, &mut offset).map_err(MigrationError::Driver)?;
        let checksum = String::decode(row, &mut offset).map_err(MigrationError::Driver)?;
        let down_checksum =
            Option::<String>::decode(row, &mut offset).map_err(MigrationError::Driver)?;
        applied.push(AppliedMigration {
            version,
            name,
            checksum,
            down_checksum,
        });
    }
    Ok(applied)
}

/// Refuse to continue when an already-applied migration no longer matches its file.
fn verify_checksums<F>(
    record: &AppliedMigration,
    migration: &Migration,
) -> Result<(), MigrationError<F>> {
    let actual = migration.checksum();
    if record.checksum != actual {
        return Err(MigrationError::ChecksumMismatch {
            version: migration.version,
            expected: record.checksum.clone(),
            actual,
        });
    }

    // A row recorded before down checksums existed has no value to compare against, so a down
    // file added later is allowed. Once a checksum is recorded, editing the file is an error.
    if let (Some(expected), Some(actual)) = (&record.down_checksum, migration.down_checksum()) {
        if *expected != actual {
            return Err(MigrationError::ChecksumMismatch {
                version: migration.version,
                expected: expected.clone(),
                actual,
            });
        }
    }
    Ok(())
}

/// The query that asks whether the journal already has its fourth column.
///
/// The dialects answer this differently and there is no portable spelling, so the dialect's
/// name picks the query. Both answers are ordinary queries, which is the point: see the call
/// site for why a failing probe is not an option.
///
/// The scope matters. `information_schema` spans every schema on PostgreSQL and every
/// database on MySQL, so an unrelated application's `_besu_migrations` must not answer for
/// this one: a `COUNT(*)` that saw its `down_checksum` would skip the `ALTER` here, and the
/// `SELECT` that follows would then fail because this journal does not have the column.
fn journal_column_probe(dialect: &str) -> &'static str {
    match dialect {
        "sqlite" => {
            "SELECT COUNT(*) FROM pragma_table_info('_besu_migrations') WHERE name = 'down_checksum'"
        }
        "mysql" => concat!(
            "SELECT COUNT(*) FROM information_schema.columns ",
            "WHERE table_schema = DATABASE() AND table_name = '_besu_migrations' ",
            "AND column_name = 'down_checksum'"
        ),
        _ => concat!(
            "SELECT COUNT(*) FROM information_schema.columns ",
            "WHERE table_schema = current_schema() AND table_name = '_besu_migrations' ",
            "AND column_name = 'down_checksum'"
        ),
    }
}

/// Whether the journal already has its fourth column.
async fn journal_has_down_checksum<E: Executor>(exec: &mut E) -> Result<bool, ErrorOf<E>>
where
    i64: Decode<E::Driver>,
{
    use besu_core::Dialect;

    let sql = journal_column_probe(<besu_core::DialectOf<E> as Dialect>::NAME);

    let rows = exec.query(&Sql::raw(sql)).await?;
    let Some(row) = rows.first() else {
        return Ok(false);
    };
    let mut offset = 0;
    let count = i64::decode(row, &mut offset)?;
    Ok(count > 0)
}

/// Split a SQL blob into statements, respecting single-quoted strings and `--` comments.
pub fn split_statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = sql.chars().peekable();

    while let Some(ch) = chars.next() {
        if let Some(q) = quote {
            current.push(ch);
            if ch == q {
                // Doubled quotes are escapes, not the end of the string.
                if chars.peek() == Some(&q) {
                    current.push(chars.next().unwrap_or(ch));
                } else {
                    quote = None;
                }
            }
            continue;
        }

        match ch {
            '\'' | '"' | '`' => {
                quote = Some(ch);
                current.push(ch);
            }
            '-' if chars.peek() == Some(&'-') => {
                // Skip to the end of the line.
                for c in chars.by_ref() {
                    if c == '\n' {
                        current.push('\n');
                        break;
                    }
                }
            }
            ';' => {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    statements.push(trimmed.to_owned());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    let trimmed = current.trim();
    if !trimmed.is_empty() {
        statements.push(trimmed.to_owned());
    }

    statements
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_statements_on_semicolons() {
        let statements = split_statements("CREATE TABLE a (id INT);\nCREATE TABLE b (id INT);");
        assert_eq!(statements.len(), 2);
        assert!(statements[0].starts_with("CREATE TABLE a"));
    }

    #[test]
    fn ignores_semicolons_inside_strings() {
        let statements = split_statements("INSERT INTO t VALUES ('a;b');");
        assert_eq!(statements, ["INSERT INTO t VALUES ('a;b')"]);
    }

    #[test]
    fn ignores_semicolons_inside_comments() {
        let statements = split_statements("-- a; b\nSELECT 1;");
        assert_eq!(statements, ["SELECT 1"]);
    }

    #[test]
    fn keeps_a_trailing_statement_without_a_semicolon() {
        let statements = split_statements("SELECT 1");
        assert_eq!(statements, ["SELECT 1"]);
    }

    #[test]
    fn the_journal_probe_is_scoped_to_this_database() {
        // `information_schema` is shared with every other schema on PostgreSQL and every
        // other database on MySQL, so an unscoped count could see another application's
        // journal and skip the `ALTER` that adds the column to this one.
        let postgres = super::journal_column_probe("postgres");
        assert!(
            postgres.contains("table_schema = current_schema()"),
            "{postgres}"
        );

        let mysql = super::journal_column_probe("mysql");
        assert!(mysql.contains("table_schema = DATABASE()"), "{mysql}");

        // SQLite has no information_schema: the pragma describes the one attached database.
        let sqlite = super::journal_column_probe("sqlite");
        assert!(
            sqlite.contains("pragma_table_info('_besu_migrations')"),
            "{sqlite}"
        );
    }

    #[test]
    fn checksum_is_stable_and_sensitive_to_content() {
        let a = Migration::from_sql(1, "init", "CREATE TABLE a (id INT)");
        let b = Migration::from_sql(1, "init", "CREATE TABLE a (id INT)");
        let c = Migration::from_sql(1, "init", "CREATE TABLE a (id BIGINT)");
        assert_eq!(a.checksum(), b.checksum());
        assert_ne!(a.checksum(), c.checksum());
    }

    #[test]
    fn up_checksum_is_pinned() {
        // Every database that has run this migration stores this value in its journal, so a
        // refactor that changes it would invalidate all of them. The test exists to make that
        // change deliberate.
        let migration = Migration::from_sql(1, "init", "CREATE TABLE a (id INT)");
        assert_eq!(migration.checksum(), "c5aaac2804fd1185");
    }

    #[test]
    fn down_checksum_covers_only_the_down_statements() {
        let migration = Migration::from_sql_pair(
            1,
            "init",
            "CREATE TABLE a (id INT)",
            Some("DROP TABLE a".to_owned()),
        );
        let edited = Migration::from_sql_pair(
            1,
            "init",
            "CREATE TABLE a (id INT)",
            Some("DROP TABLE a; DROP TABLE b".to_owned()),
        );

        assert_eq!(
            migration.down_checksum().as_deref(),
            Some("539878c5921f7627")
        );
        assert_ne!(migration.down_checksum(), edited.down_checksum());
        // The up file did not change, so its checksum must not move.
        assert_eq!(migration.checksum(), edited.checksum());
        assert_eq!(
            Migration::from_sql(1, "init", "CREATE TABLE a (id INT)").down_checksum(),
            None
        );
    }
}
