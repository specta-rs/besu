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

/// FNV-1a over a migration's name, direction and length-delimited statements.
///
/// Length prefixes make statement boundaries part of the digest. Joining statements with a
/// newline is ambiguous: replacing one separator with a newline can produce the same bytes
/// from different statement vectors.
fn checksum_of(name: &str, direction: &str, statements: &[String]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    feed_field(&mut hash, name.as_bytes());
    feed_field(&mut hash, direction.as_bytes());
    feed_bytes(&mut hash, &(statements.len() as u64).to_be_bytes());
    for statement in statements {
        feed_field(&mut hash, statement.as_bytes());
    }
    format!("{hash:016x}")
}

fn feed_field(hash: &mut u64, bytes: &[u8]) {
    feed_bytes(hash, &(bytes.len() as u64).to_be_bytes());
    feed_bytes(hash, bytes);
}

fn feed_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// The checksum format used before statement boundaries were length-delimited.
///
/// Existing journals contain these digests, so unchanged legacy migrations must still verify.
/// Newly applied migrations use the unambiguous format above.
fn legacy_checksum_of(name: &str, direction: &str, statements: &[String]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    feed_bytes(&mut hash, name.as_bytes());
    feed_bytes(&mut hash, &[0]);
    feed_bytes(&mut hash, direction.as_bytes());
    feed_bytes(&mut hash, statements.join("\n").as_bytes());
    format!("{hash:016x}")
}

fn checksum_matches(expected: &str, name: &str, direction: &str, statements: &[String]) -> bool {
    expected == checksum_of(name, direction, statements)
        || expected == legacy_checksum_of(name, direction, statements)
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
    /// The runner could not safely perform the migration.
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
    /// An applied migration's file is missing, so its history cannot be verified.
    MissingMigrationFile {
        /// The applied version with no supplied migration.
        version: i64,
    },
    /// A migration older than the newest applied version is being backfilled.
    OutOfOrderMigration {
        /// The out-of-order migration's version.
        version: i64,
        /// The newest version already in the journal.
        latest_applied: i64,
    },
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
            Self::MissingMigrationFile { version } => write!(
                f,
                "applied migration {version} is missing from the migration directory; \
                 restore its file before continuing"
            ),
            Self::OutOfOrderMigration {
                version,
                latest_applied,
            } => write!(
                f,
                "migration {version} is older than the latest applied migration \
                 {latest_applied}; migrations must be appended in ascending order"
            ),
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

        // 2. Validate: no duplicates, complete applied history, and append-only ordering.
        let mut sorted = self.migrations.clone();
        sorted.sort_by_key(|m| m.version);
        for pair in sorted.windows(2) {
            if pair[0].version == pair[1].version {
                return Err(MigrationError::DuplicateVersion(pair[0].version));
            }
        }
        // Every journal row needs its original file so its checksum can be verified. Without
        // this check a partial migration directory can silently run new SQL on top of history
        // that the runner cannot inspect.
        for record in &applied {
            if !sorted.iter().any(|m| m.version == record.version) {
                return Err(MigrationError::MissingMigrationFile {
                    version: record.version,
                });
            }
        }
        for migration in &sorted {
            if let Some(record) = applied.iter().find(|a| a.version == migration.version) {
                verify_checksums(record, migration)?;
            } else if let Some(latest) = applied.last() {
                if migration.version <= latest.version {
                    return Err(MigrationError::OutOfOrderMigration {
                        version: migration.version,
                        latest_applied: latest.version,
                    });
                }
            }
        }

        // 3. Apply the missing ones.
        let mut newly_applied = Vec::new();
        for migration in &sorted {
            if applied.iter().any(|a| a.version == migration.version) {
                continue;
            }

            check_migration_statements::<E>(migration.version, &migration.statements)?;
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
            // Zero is the CLI's revert-everything sentinel, not an ordinary version bound.
            // Versions are signed, so an inclusive comparison would leave zero and negative
            // journal rows applied forever.
            if to != 0 && newest.version <= to {
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
        check_migration_statements::<E>(record.version, statements)?;

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

/// SQLite ignores changes to foreign-key enforcement inside a transaction. In a table
/// rebuild, the subsequent DROP can therefore cascade into other tables. Even outside a
/// transaction, an executor backed by a pool cannot promise the same connection throughout.
fn check_migration_statements<E: Executor>(
    version: i64,
    statements: &[String],
) -> Result<(), MigrationError<ErrorOf<E>>> {
    use besu_core::Dialect;

    if <besu_core::DialectOf<E> as Dialect>::NAME == "sqlite"
        && statements
            .iter()
            .any(|statement| disables_sqlite_foreign_keys(statement))
    {
        return Err(MigrationError::Core(besu_core::Error::unsupported(
            format!(
                "SQLite migration {version} disables foreign_keys, but SQLite ignores that setting \
                 inside a transaction. Rewrite the migration to keep foreign_keys enabled, or run \
                 the rebuild separately on a dedicated connection with foreign_keys disabled \
                 before beginning the transaction and run PRAGMA foreign_key_check before committing."
            ),
        )));
    }
    Ok(())
}

fn disables_sqlite_foreign_keys(mut statement: &str) -> bool {
    // Recognise both PRAGMA foreign_keys = OFF and PRAGMA main.foreign_keys(OFF).
    // SQLite allows comments between tokens and quoting pragma names and values.
    fn token<'a>(statement: &mut &'a str) -> Option<&'a str> {
        loop {
            *statement =
                statement.trim_start_matches(|ch: char| ch.is_whitespace() || ch == '\u{feff}');
            if let Some(comment) = statement.strip_prefix("/*") {
                *statement = comment.split_once("*/")?.1;
            } else if let Some(comment) = statement.strip_prefix("--") {
                *statement = comment.split_once('\n')?.1;
            } else {
                break;
            }
        }
        let first = statement.chars().next()?;
        let end = match first {
            '(' | ')' | '=' | '.' | ';' => 1,
            '\'' | '"' | '`' => statement[1..].find(first)? + 2,
            '[' => statement[1..].find(']')? + 2,
            _ => statement
                .find(|ch: char| {
                    ch.is_whitespace() || matches!(ch, '(' | ')' | '=' | '.' | ';' | '/')
                })
                .unwrap_or(statement.len()),
        };
        let (token, rest) = statement.split_at(end);
        *statement = rest;
        Some(token)
    }

    if !token(&mut statement).is_some_and(|word| word.eq_ignore_ascii_case("pragma")) {
        return false;
    }
    let mut name = token(&mut statement);
    let mut separator = token(&mut statement);
    if separator == Some(".") {
        name = token(&mut statement);
        separator = token(&mut statement);
    }
    let unquote = |ch| matches!(ch, '\'' | '"' | '`' | '[' | ']');
    if !name.is_some_and(|name| {
        name.trim_matches(unquote)
            .eq_ignore_ascii_case("foreign_keys")
    }) || !matches!(separator, Some("=" | "("))
    {
        return false;
    }
    token(&mut statement).is_some_and(|value| {
        let value = value.trim_matches(unquote).to_ascii_lowercase();
        // SQLite treats unrecognised boolean values as false, not as an error.
        !matches!(value.as_str(), "on" | "yes" | "true")
            && !value.parse::<i64>().is_ok_and(|value| value > 0)
            && !value
                .strip_prefix("0x")
                .and_then(|hex| i64::from_str_radix(hex, 16).ok())
                .is_some_and(|value| value > 0)
    })
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
    if !checksum_matches(&record.checksum, &migration.name, "", &migration.statements) {
        let actual = migration.checksum();
        return Err(MigrationError::ChecksumMismatch {
            version: migration.version,
            expected: record.checksum.clone(),
            actual,
        });
    }

    // A row recorded before down checksums existed has no value to compare against, so a down
    // file added later is allowed. Once a checksum is recorded, editing the file is an error.
    if let Some(expected) = &record.down_checksum {
        let matches = migration.down.as_deref().is_some_and(|statements| {
            checksum_matches(expected, &migration.name, "down", statements)
        });
        if !matches {
            return Err(MigrationError::ChecksumMismatch {
                version: migration.version,
                expected: expected.clone(),
                actual: migration
                    .down_checksum()
                    .unwrap_or_else(|| "<missing>".to_owned()),
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

/// Split a SQL blob into statements, respecting quoted strings, `--` comments and
/// PostgreSQL dollar-quoted bodies.
pub fn split_statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut dollar_quote: Option<(String, usize)> = None;
    let mut chars = sql.chars().peekable();

    while let Some(ch) = chars.next() {
        if let Some((delimiter, body_start)) = &dollar_quote {
            current.push(ch);
            if current.len() >= *body_start + delimiter.len()
                && current.ends_with(delimiter.as_str())
            {
                dollar_quote = None;
            }
            continue;
        }

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
            '$' if current
                .chars()
                .next_back()
                .is_none_or(|previous| !is_identifier_char(previous)) =>
            {
                if let Some(delimiter) = dollar_quote_delimiter(chars.clone()) {
                    // The opening `$` has already been consumed from the iterator.
                    for _ in delimiter.chars().skip(1) {
                        chars.next();
                    }
                    current.push_str(&delimiter);
                    let body_start = current.len();
                    dollar_quote = Some((delimiter, body_start));
                } else {
                    current.push(ch);
                }
            }
            '$' => current.push(ch),
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

fn dollar_quote_delimiter(mut chars: std::iter::Peekable<std::str::Chars<'_>>) -> Option<String> {
    let mut tag = String::new();
    while let Some(&ch) = chars.peek() {
        if ch == '$' {
            let valid = match tag.chars().next() {
                None => true,
                Some(first) => {
                    is_dollar_tag_start(first) && tag.chars().all(is_dollar_tag_continue)
                }
            };
            if !valid {
                return None;
            }
            chars.next();
            return Some(format!("${tag}$"));
        }

        let valid = if tag.is_empty() {
            is_dollar_tag_start(ch)
        } else {
            is_dollar_tag_continue(ch)
        };
        if !valid {
            return None;
        }
        tag.push(ch);
        chars.next();
    }
    None
}

fn is_dollar_tag_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic() || !ch.is_ascii()
}

fn is_identifier_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric() || !ch.is_ascii()
}

fn is_dollar_tag_continue(ch: char) -> bool {
    is_dollar_tag_start(ch) || ch.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_sqlite_foreign_key_disable_statements() {
        for statement in [
            "PRAGMA foreign_keys = OFF",
            "pragma main.foreign_keys(0)",
            "/* rebuild */ PRAGMA /* keep */ [foreign_keys] = 'no'",
            "pragma \"foreign_keys\"(false)",
        ] {
            assert!(disables_sqlite_foreign_keys(statement), "{statement}");
        }
        for statement in [
            "PRAGMA foreign_keys",
            "PRAGMA foreign_keys = ON",
            "PRAGMA foreign_keys(1)",
            "PRAGMA defer_foreign_keys = OFF",
            "SELECT 'PRAGMA foreign_keys = OFF'",
        ] {
            assert!(!disables_sqlite_foreign_keys(statement), "{statement}");
        }
    }

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
    fn keeps_postgres_dollar_quoted_bodies_in_one_statement() {
        let sql = "CREATE FUNCTION public.answer()\n\
                   RETURNS integer\n\
                   LANGUAGE plpgsql\n\
                   AS $$\n\
                   BEGIN\n\
                       RETURN 1;\n\
                   END;\n\
                   $$;\n\
                   SELECT public.answer();";
        let statements = split_statements(sql);
        assert_eq!(statements.len(), 2, "{statements:#?}");
        assert!(statements[0].starts_with("CREATE FUNCTION public.answer()"));
        assert!(statements[0].ends_with("$$"));
        assert_eq!(statements[1], "SELECT public.answer()");
    }

    #[test]
    fn dollar_quote_tags_are_case_sensitive_and_self_delimited() {
        let statements = split_statements(
            "CREATE FUNCTION f() RETURNS void AS $Body$ BEGIN PERFORM 1; END; $Body$;\n\
             CREATE FUNCTION g() RETURNS void AS $body$ BEGIN PERFORM 2; END; $body$;",
        );
        assert_eq!(statements.len(), 2, "{statements:#?}");
        assert!(statements[0].ends_with("$Body$"));
        assert!(statements[1].ends_with("$body$"));
    }

    #[test]
    fn does_not_treat_parameters_as_dollar_quotes() {
        let statements = split_statements("SELECT $1; SELECT $tag; SELECT 2;");
        assert_eq!(statements, ["SELECT $1", "SELECT $tag", "SELECT 2"]);
    }

    #[test]
    fn does_not_treat_dollar_quotes_inside_identifiers_as_openers() {
        for (sql, identifier) in [
            ("SELECT a$b$; SELECT 2;", "SELECT a$b$"),
            ("SELECT 1$b$; SELECT 2;", "SELECT 1$b$"),
            ("SELECT _$b$; SELECT 2;", "SELECT _$b$"),
            ("SELECT é$b$; SELECT 2;", "SELECT é$b$"),
            ("SELECT a$$; SELECT 2;", "SELECT a$$"),
        ] {
            assert_eq!(split_statements(sql), [identifier, "SELECT 2"], "{sql}");
        }
    }

    #[test]
    fn ignores_dollar_quote_openers_inside_regular_strings() {
        let statements = split_statements("SELECT '$$;', '$tag$;'; SELECT 2;");
        assert_eq!(statements, ["SELECT '$$;', '$tag$;'", "SELECT 2"]);
    }

    #[test]
    fn dollar_quoted_bodies_can_contain_multibyte_text() {
        let statements = split_statements("SELECT $$€;$$; SELECT 2;");
        assert_eq!(statements, ["SELECT $$€;$$", "SELECT 2"]);
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
        // New journals store the length-delimited format. Legacy digests are pinned below and
        // still accepted when verifying existing history.
        let migration = Migration::from_sql(1, "init", "CREATE TABLE a (id INT)");
        assert_eq!(migration.checksum(), "70cefc2f1aca861d");
    }

    #[test]
    fn legacy_up_checksum_is_pinned() {
        let migration = Migration::from_sql(1, "init", "CREATE TABLE a (id INT)");
        assert_eq!(
            legacy_checksum_of(&migration.name, "", &migration.statements),
            "c5aaac2804fd1185"
        );
    }

    #[test]
    fn identifier_dollar_text_preserves_legacy_statement_boundaries() {
        let migration = Migration::from_sql(1, "init", "SELECT a$b$; SELECT 2;");
        assert_eq!(migration.statements, ["SELECT a$b$", "SELECT 2"]);
        assert_eq!(
            legacy_checksum_of(&migration.name, "", &migration.statements),
            "9c51c8f9916036c8"
        );

        let record = AppliedMigration {
            version: 1,
            name: "init".to_owned(),
            checksum: "9c51c8f9916036c8".to_owned(),
            down_checksum: None,
        };
        verify_checksums::<besu_core::Error>(&record, &migration)
            .expect("identifier dollar text must keep legacy checksums valid");
    }

    #[test]
    fn checksum_changes_when_only_statement_boundaries_change() {
        let before = Migration::from_sql(
            1,
            "init",
            "CREATE TABLE a (id INT);\nCREATE TABLE b (id INT);",
        );
        let after = Migration::from_sql(
            1,
            "init",
            "CREATE TABLE a (id INT)\nCREATE TABLE b (id INT);",
        );
        assert_eq!(before.statements.len(), 2);
        assert_eq!(after.statements.len(), 1);
        assert_eq!(before.statements.join("\n"), after.statements.join("\n"));
        assert_ne!(before.checksum(), after.checksum());
    }

    #[test]
    fn unchanged_legacy_up_and_down_checksums_still_verify() {
        let migration = Migration::from_sql_pair(
            1,
            "init",
            "CREATE TABLE a (id INT)",
            Some("DROP TABLE a".to_owned()),
        );
        let record = AppliedMigration {
            version: 1,
            name: "init".to_owned(),
            checksum: legacy_checksum_of(&migration.name, "", &migration.statements),
            down_checksum: Some(legacy_checksum_of(
                &migration.name,
                "down",
                migration
                    .down
                    .as_deref()
                    .expect("test migration has down statements"),
            )),
        };
        verify_checksums::<besu_core::Error>(&record, &migration)
            .expect("unchanged legacy checksums should verify");

        let edited = Migration::from_sql_pair(
            1,
            "init",
            "CREATE TABLE a (id BIGINT)",
            Some("DROP TABLE a".to_owned()),
        );
        assert!(matches!(
            verify_checksums::<besu_core::Error>(&record, &edited),
            Err(MigrationError::ChecksumMismatch { version: 1, .. })
        ));
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
            Some("06e7fb8e93f3a4be")
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
