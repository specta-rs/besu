//! The migration runner, end to end against a real database.
#![cfg(feature = "sqlx")]
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::prelude::*;
use besu::schema::{Migration, MigrationError, Migrator};
use besu_sqlite::Sqlx;

#[tokio::test]
async fn applies_each_migration_once() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("0001_create_users.sql"),
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL);",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("0002_add_email.sql"),
        "ALTER TABLE users ADD COLUMN email TEXT;\n\
         CREATE UNIQUE INDEX users_email_idx ON users (email);",
    )
    .unwrap();

    let migrator = Migrator::from_dir(dir.path()).unwrap();
    assert_eq!(migrator.len(), 2);

    // A file database pools several connections, which is what exposed the bug where two
    // statements of one schema change ran on different connections.
    let path = dir.path().join("app.db");
    let db = besu::Database::new(Sqlx::new(&path).await.unwrap());

    let applied = db.migrate(&migrator).await.unwrap();
    assert_eq!(
        applied.iter().map(|m| m.version).collect::<Vec<_>>(),
        [1, 2]
    );

    // Running again applies nothing: the journal remembers what has run.
    let applied = db.migrate(&migrator).await.unwrap();
    assert!(applied.is_empty());

    // The migration's effects are real.
    db.execute(sql!(
        "INSERT INTO users (id, name, email) VALUES (?, ?, ?)",
        1,
        "Oscar",
        "o@example.com"
    ))
    .await
    .unwrap();
    let rows = db
        .execute(sql!(
            "SELECT name FROM users WHERE email = ?",
            "o@example.com"
        ))
        .all::<(String,)>()
        .await
        .unwrap();
    assert_eq!(rows, [("Oscar".to_string(),)]);
}

#[tokio::test]
async fn refuses_to_run_a_sqlite_rebuild_that_disables_foreign_keys() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    let mut migrator = Migrator::new();
    migrator.push(Migration::from_sql(
        1,
        "seed",
        "CREATE TABLE parents (id INTEGER PRIMARY KEY, note TEXT);
         CREATE TABLE children (
             id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES parents(id) ON DELETE CASCADE
         );
         INSERT INTO parents VALUES (1, 'note');
         INSERT INTO children VALUES (1, 1);",
    ));
    db.migrate(&migrator).await.unwrap();
    migrator.push(Migration::from_sql(
        2,
        "rebuild parents",
        "PRAGMA foreign_keys = OFF;
         CREATE TABLE parents__besu_tmp (id INTEGER PRIMARY KEY, note TEXT NOT NULL);
         INSERT INTO parents__besu_tmp SELECT id, note FROM parents;
         DROP TABLE parents;
         ALTER TABLE parents__besu_tmp RENAME TO parents;
         PRAGMA foreign_keys = ON;",
    ));

    let result = db.migrate(&migrator).await;
    let children = db
        .execute(sql!("SELECT id FROM children"))
        .all::<(i64,)>()
        .await
        .unwrap();
    assert_eq!(
        children,
        [(1,)],
        "the rebuild must not cascade into the child"
    );
    assert!(
        result.unwrap_err().to_string().contains("foreign_keys"),
        "the error must explain why the rebuild was refused"
    );
    assert_eq!(journal_len(&db).await, 1);
}

#[tokio::test]
async fn refuses_to_continue_when_an_applied_migration_changed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("0001_create_users.sql");
    std::fs::write(&path, "CREATE TABLE users (id INTEGER PRIMARY KEY);").unwrap();

    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    let migrator = Migrator::from_dir(dir.path()).unwrap();
    db.migrate(&migrator).await.unwrap();

    // Editing history is a mistake the runner should catch, not tolerate.
    std::fs::write(
        &path,
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);",
    )
    .unwrap();
    let migrator = Migrator::from_dir(dir.path()).unwrap();
    let error = db.migrate(&migrator).await.unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("append-only"),
        "expected a checksum mismatch, got: {message}"
    );
}

#[tokio::test]
async fn migration_journal_is_not_part_of_the_schema() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("0001_create_users.sql"),
        "CREATE TABLE users (id INTEGER PRIMARY KEY);",
    )
    .unwrap();

    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.migrate(&Migrator::from_dir(dir.path()).unwrap())
        .await
        .unwrap();

    let schema = db.introspect().await.unwrap();
    assert!(schema.table("users").is_some());
    assert!(
        schema.table("_besu_migrations").is_none(),
        "the journal must not show up in the schema, or push would try to drop it"
    );
}

/// The pair of files used by most down tests: `users`, then an indexed `email` column.
fn write_users_migrations(dir: &std::path::Path, down: bool) {
    std::fs::write(
        dir.join("0001_create_users.sql"),
        "CREATE TABLE users (id INTEGER PRIMARY KEY);",
    )
    .unwrap();
    std::fs::write(
        dir.join("0002_add_email.sql"),
        "ALTER TABLE users ADD COLUMN email TEXT;\n\
         CREATE UNIQUE INDEX users_email_idx ON users (email);",
    )
    .unwrap();
    if down {
        std::fs::write(dir.join("0001_create_users.down.sql"), "DROP TABLE users;").unwrap();
        // A down file mirrors its up file: the runner walks it from the bottom up, so the
        // index goes before the column it covers.
        std::fs::write(
            dir.join("0002_add_email.down.sql"),
            "ALTER TABLE users DROP COLUMN email;\nDROP INDEX users_email_idx;",
        )
        .unwrap();
    }
}

async fn journal_len(db: &besu::Database<Sqlx>) -> i64 {
    db.execute(sql!("SELECT COUNT(*) FROM _besu_migrations"))
        .all::<(i64,)>()
        .await
        .unwrap()[0]
        .0
}

#[tokio::test]
async fn reverts_the_newest_migration() {
    let dir = tempfile::tempdir().unwrap();
    write_users_migrations(dir.path(), true);

    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    let migrator = Migrator::from_dir(dir.path()).unwrap();
    db.migrate(&migrator).await.unwrap();
    assert_eq!(journal_len(&db).await, 2);

    let reverted = db.migrate_down_one(&migrator).await.unwrap().unwrap();
    assert_eq!(reverted.version, 2);
    assert_eq!(reverted.name, "add email");
    assert_eq!(journal_len(&db).await, 1);

    // The column and its index are gone; the table it belonged to is not.
    let schema = db.introspect().await.unwrap();
    let users = schema.table("users").unwrap();
    assert!(
        users.columns.iter().all(|column| column.name != "email"),
        "the down migration should have dropped the column: {users:?}"
    );
    assert!(users.indexes.is_empty(), "the index should be gone too");

    // Reverting again walks back one more version.
    let reverted = db.migrate_down_one(&migrator).await.unwrap().unwrap();
    assert_eq!(reverted.version, 1);
    assert!(db.introspect().await.unwrap().table("users").is_none());
    assert_eq!(journal_len(&db).await, 0);

    // An empty journal has nothing left to revert, and says so rather than failing.
    assert!(db.migrate_down_one(&migrator).await.unwrap().is_none());
}

#[tokio::test]
async fn refuses_to_continue_when_only_statement_boundaries_changed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("0001_init.sql");
    std::fs::write(&path, "CREATE TABLE a (id INT);\nCREATE TABLE b (id INT);").unwrap();

    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.migrate(&Migrator::from_dir(dir.path()).unwrap())
        .await
        .unwrap();

    // The old checksum joined statements with a newline, so these two files had the same
    // digest even though the second is one invalid statement instead of two valid ones.
    std::fs::write(&path, "CREATE TABLE a (id INT)\nCREATE TABLE b (id INT);").unwrap();
    let error = db
        .migrate(&Migrator::from_dir(dir.path()).unwrap())
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            besu::SchemaError::Migration(MigrationError::ChecksumMismatch { version: 1, .. })
        ),
        "unexpected error: {error}"
    );
    assert_eq!(journal_len(&db).await, 1);
    let schema = db.introspect().await.unwrap();
    assert!(schema.table("a").is_some());
    assert!(schema.table("b").is_some());
}

#[tokio::test]
async fn reverts_everything_with_to_zero() {
    let dir = tempfile::tempdir().unwrap();
    write_users_migrations(dir.path(), true);

    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    let migrator = Migrator::from_dir(dir.path()).unwrap();
    db.migrate(&migrator).await.unwrap();

    let reverted = db.migrate_down(&migrator, 0).await.unwrap();
    assert_eq!(
        reverted.iter().map(|m| m.version).collect::<Vec<_>>(),
        [2, 1],
        "reverts come back newest first"
    );
    assert_eq!(journal_len(&db).await, 0);
    assert!(db.introspect().await.unwrap().table("users").is_none());

    // Reverting nothing is not an error.
    assert!(db.migrate_down(&migrator, 0).await.unwrap().is_empty());
}

#[tokio::test]
async fn reverts_down_to_a_version_and_stops_there() {
    let dir = tempfile::tempdir().unwrap();
    write_users_migrations(dir.path(), true);

    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    let migrator = Migrator::from_dir(dir.path()).unwrap();
    db.migrate(&migrator).await.unwrap();

    let reverted = db.migrate_down(&migrator, 1).await.unwrap();
    assert_eq!(reverted.iter().map(|m| m.version).collect::<Vec<_>>(), [2]);
    assert_eq!(journal_len(&db).await, 1);
    assert!(db.introspect().await.unwrap().table("users").is_some());
}

async fn journal_versions(db: &besu::Database<Sqlx>) -> Vec<i64> {
    db.execute(sql!(
        "SELECT version FROM _besu_migrations ORDER BY version"
    ))
    .all::<(i64,)>()
    .await
    .unwrap()
    .into_iter()
    .map(|(version,)| version)
    .collect()
}

#[tokio::test]
async fn refuses_to_revert_a_migration_without_a_down_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("0001_create_users.sql"),
        "CREATE TABLE users (id INTEGER PRIMARY KEY);",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("0001_create_users.down.sql"),
        "DROP TABLE users;",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("0002_add_email.sql"),
        "ALTER TABLE users ADD COLUMN email TEXT;",
    )
    .unwrap();

    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    let migrator = Migrator::from_dir(dir.path()).unwrap();
    db.migrate(&migrator).await.unwrap();

    let error = db.migrate_down_one(&migrator).await.unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("migration 2 has no down file"),
        "expected a missing down file, got: {message}"
    );

    // Nothing was reverted: the journal and the schema are untouched.
    assert_eq!(journal_len(&db).await, 2);
    let schema = db.introspect().await.unwrap();
    assert!(
        schema
            .table("users")
            .unwrap()
            .columns
            .iter()
            .any(|column| column.name == "email")
    );
}

#[tokio::test]
async fn reverts_zero_and_negative_versions_with_to_zero() {
    let dir = tempfile::tempdir().unwrap();
    write_version_pair(dir.path(), -7, "legacy", "sentinel_negative");
    write_version_pair(dir.path(), 0, "init", "sentinel_zero");
    write_version_pair(dir.path(), 2, "positive", "sentinel_positive");

    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    let migrator = Migrator::from_dir(dir.path()).unwrap();
    db.migrate(&migrator).await.unwrap();
    assert_eq!(journal_versions(&db).await, [-7, 0, 2]);

    let reverted = db.migrate_down(&migrator, 0).await.unwrap();
    assert_eq!(
        reverted.iter().map(|m| m.version).collect::<Vec<_>>(),
        [2, 0, -7]
    );
    assert!(journal_versions(&db).await.is_empty());
    let schema = db.introspect().await.unwrap();
    assert!(schema.table("sentinel_negative").is_none());
    assert!(schema.table("sentinel_zero").is_none());
    assert!(schema.table("sentinel_positive").is_none());
}

#[tokio::test]
async fn an_ordinary_down_target_keeps_zero_and_negative_versions() {
    let dir = tempfile::tempdir().unwrap();
    write_version_pair(dir.path(), -7, "legacy", "sentinel_negative");
    write_version_pair(dir.path(), 0, "init", "sentinel_zero");
    write_version_pair(dir.path(), 2, "positive", "sentinel_positive");

    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    let migrator = Migrator::from_dir(dir.path()).unwrap();
    db.migrate(&migrator).await.unwrap();

    let reverted = db.migrate_down(&migrator, 1).await.unwrap();
    assert_eq!(reverted.len(), 1);
    assert_eq!(reverted[0].version, 2);
    assert_eq!(journal_versions(&db).await, [-7, 0]);
    let schema = db.introspect().await.unwrap();
    assert!(schema.table("sentinel_negative").is_some());
    assert!(schema.table("sentinel_zero").is_some());
    assert!(schema.table("sentinel_positive").is_none());
}

fn write_version_pair(dir: &std::path::Path, version: i64, name: &str, table: &str) {
    let prefix = if version < 0 {
        format!("-{:04}", version.unsigned_abs())
    } else {
        format!("{version:04}")
    };
    std::fs::write(
        dir.join(format!("{prefix}_{name}.sql")),
        format!("CREATE TABLE {table} (id INTEGER PRIMARY KEY);"),
    )
    .unwrap();
    std::fs::write(
        dir.join(format!("{prefix}_{name}.down.sql")),
        format!("DROP TABLE {table};"),
    )
    .unwrap();
}

#[tokio::test]
async fn refuses_to_revert_when_the_down_file_changed() {
    let dir = tempfile::tempdir().unwrap();
    write_users_migrations(dir.path(), true);

    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    let migrator = Migrator::from_dir(dir.path()).unwrap();
    db.migrate(&migrator).await.unwrap();

    // The up files are untouched; only the down file was edited after it ran.
    std::fs::write(
        dir.path().join("0002_add_email.down.sql"),
        "ALTER TABLE users DROP COLUMN email;\nDROP INDEX users_email_idx;\nDROP TABLE users;",
    )
    .unwrap();

    let migrator = Migrator::from_dir(dir.path()).unwrap();
    let error = db.migrate_down(&migrator, 0).await.unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("append-only"),
        "expected a down checksum mismatch, got: {message}"
    );
    assert_eq!(journal_len(&db).await, 2);
}

#[tokio::test]
async fn runs_down_statements_in_reverse_order() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("0001_create_notes.sql"),
        "CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT);\n\
         CREATE TABLE revert_log (id INTEGER PRIMARY KEY AUTOINCREMENT, step TEXT NOT NULL);",
    )
    .unwrap();
    // The file mirrors the up file; the runner walks it from the bottom, so `notes` is dropped
    // last and the log records the two steps in the order they actually ran.
    std::fs::write(
        dir.path().join("0001_create_notes.down.sql"),
        "DROP TABLE notes;\n\
         INSERT INTO revert_log (step) VALUES ('notes dropped');\n\
         INSERT INTO revert_log (step) VALUES ('before notes dropped');",
    )
    .unwrap();

    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    let migrator = Migrator::from_dir(dir.path()).unwrap();
    db.migrate(&migrator).await.unwrap();
    db.migrate_down(&migrator, 0).await.unwrap();

    let steps = db
        .execute(sql!("SELECT step FROM revert_log ORDER BY id"))
        .all::<(String,)>()
        .await
        .unwrap();
    assert_eq!(
        steps,
        [
            ("before notes dropped".to_string(),),
            ("notes dropped".to_string(),)
        ],
        "down statements must run from the bottom of the file up"
    );
}

#[tokio::test]
async fn upgrades_a_three_column_journal() {
    let dir = tempfile::tempdir().unwrap();
    write_users_migrations(dir.path(), true);

    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    // Exactly what an older Besu created, before down migrations existed.
    db.execute(sql!(
        "CREATE TABLE _besu_migrations (\
         version BIGINT PRIMARY KEY, \
         name VARCHAR(255) NOT NULL, \
         checksum VARCHAR(64) NOT NULL)"
    ))
    .await
    .unwrap();

    let migrator = Migrator::from_dir(dir.path()).unwrap();
    let applied = db.migrate(&migrator).await.unwrap();
    assert_eq!(applied.len(), 2);
    assert!(
        applied.iter().all(|m| m.down_checksum.is_some()),
        "the upgraded journal must record down checksums"
    );

    // The column is there, and the down path works on the upgraded table.
    let rows = db
        .execute(sql!(
            "SELECT down_checksum FROM _besu_migrations ORDER BY version"
        ))
        .all::<(Option<String>,)>()
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|(checksum,)| checksum.is_some()));

    db.migrate_down_one(&migrator).await.unwrap().unwrap();
    db.migrate_down_one(&migrator).await.unwrap().unwrap();
    assert_eq!(journal_len(&db).await, 0);
}

#[tokio::test]
async fn accepts_an_unchanged_migration_with_a_legacy_checksum() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("0001_init.sql"), "CREATE TABLE a (id INT)").unwrap();

    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.execute(sql!(
        "CREATE TABLE _besu_migrations (\
         version BIGINT PRIMARY KEY, \
         name VARCHAR(255) NOT NULL, \
         checksum VARCHAR(64) NOT NULL, \
         down_checksum VARCHAR(64))"
    ))
    .await
    .unwrap();
    // The digest produced by the old statement-joining checksum format.
    db.execute(sql!(
        "INSERT INTO _besu_migrations (version, name, checksum) VALUES (?, ?, ?)",
        1,
        "init",
        "c5aaac2804fd1185"
    ))
    .await
    .unwrap();
    db.execute(sql!("CREATE TABLE a (id INT)")).await.unwrap();

    let applied = db
        .migrate(&Migrator::from_dir(dir.path()).unwrap())
        .await
        .unwrap();
    assert!(applied.is_empty());
    assert_eq!(journal_versions(&db).await, [1]);
}

#[tokio::test]
async fn upgrades_a_three_column_journal_that_already_has_history() {
    let dir = tempfile::tempdir().unwrap();
    write_users_migrations(dir.path(), true);

    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.execute(sql!(
        "CREATE TABLE _besu_migrations (\
         version BIGINT PRIMARY KEY, \
         name VARCHAR(255) NOT NULL, \
         checksum VARCHAR(64) NOT NULL)"
    ))
    .await
    .unwrap();

    // A version applied by the old runner: the checksum matches, but there is no down
    // checksum to compare, so the down file added since is accepted.
    let migrator = Migrator::from_dir(dir.path()).unwrap();
    let checksum = migrator.migrations()[0].checksum();
    db.execute(sql!(
        "INSERT INTO _besu_migrations (version, name, checksum) VALUES (?, ?, ?)",
        1,
        "create users",
        checksum
    ))
    .await
    .unwrap();
    db.execute(sql!("CREATE TABLE users (id INTEGER PRIMARY KEY)"))
        .await
        .unwrap();

    let applied = db.migrate(&migrator).await.unwrap();
    assert_eq!(
        applied.iter().map(|m| m.version).collect::<Vec<_>>(),
        [2],
        "version 1 is already applied"
    );

    let reverted = db.migrate_down(&migrator, 0).await.unwrap();
    assert_eq!(
        reverted.iter().map(|m| m.version).collect::<Vec<_>>(),
        [2, 1]
    );
    assert_eq!(journal_len(&db).await, 0);
}

#[tokio::test]
async fn refuses_an_up_to_date_run_when_a_recorded_down_file_was_deleted() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("0001_init.sql"),
        "CREATE TABLE sentinel (id INTEGER PRIMARY KEY);",
    )
    .unwrap();
    let down_path = dir.path().join("0001_init.down.sql");
    std::fs::write(&down_path, "DROP TABLE sentinel;").unwrap();

    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    let migrator = Migrator::from_dir(dir.path()).unwrap();
    let applied = db.migrate(&migrator).await.unwrap();
    assert!(applied[0].down_checksum.is_some());
    let recorded = db
        .execute(sql!(
            "SELECT checksum, down_checksum FROM _besu_migrations WHERE version = 1"
        ))
        .all::<(String, Option<String>)>()
        .await
        .unwrap();

    std::fs::remove_file(&down_path).unwrap();
    let migrator = Migrator::from_dir(dir.path()).unwrap();
    let error = db.migrate(&migrator).await.unwrap_err();
    assert!(
        matches!(
            error,
            besu::SchemaError::Migration(MigrationError::ChecksumMismatch {
                version: 1,
                ref actual,
                ..
            }) if actual == "<missing>"
        ),
        "unexpected error: {error}"
    );

    assert_eq!(journal_versions(&db).await, [1]);
    assert_eq!(
        db.execute(sql!(
            "SELECT checksum, down_checksum FROM _besu_migrations WHERE version = 1"
        ))
        .all::<(String, Option<String>)>()
        .await
        .unwrap(),
        recorded
    );
    assert!(db.introspect().await.unwrap().table("sentinel").is_some());
    assert!(matches!(
        db.migrate_down_one(&migrator).await.unwrap_err(),
        besu::SchemaError::Migration(MigrationError::MissingDownFile { version: 1 })
    ));
}

#[tokio::test]
async fn pairs_down_files_with_their_up_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("0001_create_users.sql"),
        "CREATE TABLE users (id INTEGER PRIMARY KEY);",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("0001_create_users.down.sql"),
        "DROP TABLE users;",
    )
    .unwrap();

    let migrator = Migrator::from_dir(dir.path()).unwrap();
    assert_eq!(
        migrator.len(),
        1,
        "a down file is not a migration of its own"
    );
    let migration = migrator.migrations()[0];
    assert_eq!(migration.down, Some(vec!["DROP TABLE users".to_owned()]));
    assert!(migration.down_checksum().is_some());

    // A down file with nothing to revert is an error, not a silently ignored file.
    std::fs::remove_file(dir.path().join("0001_create_users.sql")).unwrap();
    let error = Migrator::from_dir(dir.path()).unwrap_err();
    assert!(
        error.to_string().contains("no matching up migration"),
        "unexpected error: {error}"
    );
}
