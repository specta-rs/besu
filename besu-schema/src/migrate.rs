use core::fmt;

use besu_core::{Decode, Driver, ErrorOf, Executor, Sql};

/// One migration: a version, a human name and the SQL to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migration {
    /// Monotonically increasing version. Migrations are applied in this order.
    pub version: i64,
    /// A short human-readable name, used for the filename and the journal.
    pub name: String,
    /// One or more SQL statements.
    pub statements: Vec<String>,
}

impl Migration {
    /// Build a migration from a single SQL blob, splitting statements on `;`.
    pub fn from_sql(version: i64, name: impl Into<String>, sql: impl Into<String>) -> Self {
        Self {
            version,
            name: name.into(),
            statements: split_statements(&sql.into()),
        }
    }

    /// A stable checksum of the migration's contents.
    ///
    /// Changing an already-applied migration is almost always a mistake, so the runner
    /// refuses to continue when the checksum no longer matches.
    pub fn checksum(&self) -> String {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in self
            .name
            .bytes()
            .chain(core::iter::once(0))
            .chain(self.statements.join("\n").bytes())
        {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{hash:016x}")
    }
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
    pub fn migrations(&mut self) -> &[Migration] {
        self.migrations.sort_by_key(|m| m.version);
        &self.migrations
    }

    /// Read `*.sql` files from a directory.
    ///
    /// Files are named `<version>_<name>.sql`, for example `0007_add_email_index.sql`.
    #[cfg(feature = "fs")]
    pub fn from_dir(dir: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let mut migrations = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("sql") {
                continue;
            }
            let file_name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_owned();
            let (version, name) = match file_name.split_once('_') {
                Some((version, name)) => (version, name),
                None => (file_name.as_str(), ""),
            };
            let Ok(version) = version.parse::<i64>() else {
                continue;
            };
            let sql = std::fs::read_to_string(&path)?;
            migrations.push(Migration::from_sql(version, name.replace('_', " "), sql));
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
        Option<String>: Decode<E::Driver>,
    {
        let driver_err = <E::Driver as Driver>::error;

        // 1. Make sure the journal exists. This statement is portable across the three
        //    supported databases on purpose: the journal must be readable before any
        //    dialect-specific machinery is involved.
        let mut create = Sql::raw(concat!(
            "CREATE TABLE IF NOT EXISTS _besu_migrations (",
            "version BIGINT PRIMARY KEY, ",
            "name VARCHAR(255) NOT NULL, ",
            "checksum VARCHAR(64) NOT NULL",
            ")"
        ));
        exec.execute(&mut create).await.map_err(MigrationError::Driver)?;

        // 2. Read what has already run.
        let mut select = Sql::raw(concat!(
            "SELECT version, name, checksum FROM _besu_migrations ORDER BY version"
        ));
        let rows = exec
            .query(&mut select)
            .await
            .map_err(MigrationError::Driver)?;

        let mut applied: Vec<AppliedMigration> = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut offset = 0;
            let version = i64::decode(row, &mut offset).map_err(MigrationError::Driver)?;
            let name = String::decode(row, &mut offset).map_err(MigrationError::Driver)?;
            let checksum = String::decode(row, &mut offset).map_err(MigrationError::Driver)?;
            applied.push(AppliedMigration {
                version,
                name,
                checksum,
            });
        }

        // 3. Validate: no duplicates, and nothing that already ran was edited.
        let mut sorted = self.migrations.clone();
        sorted.sort_by_key(|m| m.version);
        for pair in sorted.windows(2) {
            if pair[0].version == pair[1].version {
                return Err(MigrationError::DuplicateVersion(pair[0].version));
            }
        }
        for migration in &sorted {
            if let Some(record) = applied.iter().find(|a| a.version == migration.version) {
                let actual = migration.checksum();
                if record.checksum != actual {
                    return Err(MigrationError::ChecksumMismatch {
                        version: migration.version,
                        expected: record.checksum.clone(),
                        actual,
                    });
                }
            }
        }

        // 4. Apply the missing ones.
        let mut newly_applied = Vec::new();
        for migration in &sorted {
            if applied.iter().any(|a| a.version == migration.version) {
                continue;
            }

            for statement in &migration.statements {
                let mut sql = Sql::<besu_core::executor::DialectOf<E>>::raw(statement.clone());
                match exec.execute(&mut sql).await {
                    Ok(_) => {}
                    Err(err) => return Err(MigrationError::Driver(err)),
                }
            }

            let mut insert = Sql::<besu_core::executor::DialectOf<E>>::raw(concat!(
                "INSERT INTO _besu_migrations (version, name, checksum) VALUES ("
            ));
            insert.bind(migration.version);
            insert.push_raw(", ");
            insert.bind(migration.name.clone());
            insert.push_raw(", ");
            insert.bind(migration.checksum());
            insert.push_raw(")");
            exec.execute(&mut insert)
                .await
                .map_err(MigrationError::Driver)?;

            newly_applied.push(AppliedMigration {
                version: migration.version,
                name: migration.name.clone(),
                checksum: migration.checksum(),
            });
        }

        let _ = driver_err;
        Ok(newly_applied)
    }
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
