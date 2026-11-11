//! The migration runner, end to end against a real database.
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::prelude::*;
use besu::schema::Migrator;
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

    let db = besu::Database::new(Sqlx::memory().await.unwrap());

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
