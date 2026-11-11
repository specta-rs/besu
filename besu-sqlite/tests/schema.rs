//! Schema fidelity on SQLite: the objects the IR cannot express are skipped by
//! introspection, and a declared schema that matches the database converges.
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::prelude::*;
use besu_sqlite::Sqlx;

/// A file-backed database, because a pool over `:memory:` would give every statement its
/// own empty database.
async fn database(dir: &tempfile::TempDir) -> besu::Database<Sqlx> {
    let path = dir.path().join("app.db");
    besu::Database::new(Sqlx::new(&path).await.unwrap())
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "events")]
pub struct Event {
    #[besu(primary_key)]
    pub id: i64,
    pub kind: String,
    pub payload: String,
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "users")]
pub struct User {
    #[besu(primary_key)]
    pub id: i64,
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "accounts")]
pub struct Account {
    #[besu(primary_key)]
    pub id: i64,
    pub balance: i64,
}

/// SQLite has partial indexes, and expression indexes have existed for as long as partial
/// ones. The IR models neither a predicate nor an expression key here, so both are skipped
/// rather than reported as plain indexes that `push` would then drop.
#[tokio::test]
async fn partial_and_expression_indexes_are_not_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let db = database(&dir).await;

    db.execute(sql!(
        "CREATE TABLE events (\
             id INTEGER PRIMARY KEY, \
             kind TEXT NOT NULL, \
             payload TEXT NOT NULL)"
    ))
    .await
    .unwrap();
    db.execute(sql!(
        "CREATE INDEX events_kind_idx ON events (kind) WHERE kind = 'error'"
    ))
    .await
    .unwrap();
    db.execute(sql!(
        "CREATE INDEX events_lower_payload_idx ON events (lower(payload))"
    ))
    .await
    .unwrap();

    let introspected = db.introspect().await.unwrap();
    assert!(
        introspected.table("events").unwrap().indexes.is_empty(),
        "neither index can be modelled and both must be skipped: {:?}",
        introspected.table("events").unwrap().indexes
    );

    let pushed = db.push([events.def()]).await.unwrap();
    assert!(pushed.is_empty(), "got {:?}", pushed.changes);
    assert!(
        !pushed
            .statements
            .iter()
            .any(|sql| sql.contains("DROP INDEX")),
        "got {:?}",
        pushed.statements
    );
}

/// A `CHECK` constraint exists only inside the `CREATE TABLE` text on SQLite, so it is not
/// introspected. Pushing the declared schema must not try to remove one.
#[tokio::test]
async fn a_check_constraint_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let db = database(&dir).await;

    db.execute(sql!(
        "CREATE TABLE accounts (\
             id INTEGER PRIMARY KEY, \
             balance INTEGER NOT NULL CHECK (balance >= 0))"
    ))
    .await
    .unwrap();

    let introspected = db.introspect().await.unwrap();
    assert!(introspected.table("accounts").unwrap().checks.is_empty());

    let pushed = db.push([accounts.def()]).await.unwrap();
    assert!(pushed.is_empty(), "got {:?}", pushed.changes);
    assert!(
        !pushed.statements.iter().any(|sql| sql.contains("CHECK")),
        "got {:?}",
        pushed.statements
    );
}

/// A view is reported so `besu pull` writes it down, but it is never a table and the differ
/// never creates or drops one.
#[tokio::test]
async fn a_view_is_reported_but_never_touched() {
    let dir = tempfile::tempdir().unwrap();
    let db = database(&dir).await;

    db.execute(sql!("CREATE TABLE users (id INTEGER PRIMARY KEY)"))
        .await
        .unwrap();
    db.execute(sql!("CREATE VIEW user_ids AS SELECT id FROM users"))
        .await
        .unwrap();

    let introspected = db.introspect().await.unwrap();
    assert!(
        introspected.table("user_ids").is_none(),
        "a view must not be reported as a table"
    );
    assert_eq!(introspected.views.len(), 1);
    assert_eq!(introspected.views[0].name, "user_ids");
    assert!(
        introspected.views[0]
            .definition
            .to_ascii_uppercase()
            .contains("SELECT"),
        "got {:?}",
        introspected.views[0].definition
    );

    let pushed = db.push([users.def()]).await.unwrap();
    assert!(pushed.is_empty(), "got {:?}", pushed.changes);
    assert!(
        !pushed
            .statements
            .iter()
            .any(|sql| sql.to_uppercase().contains("VIEW")),
        "got {:?}",
        pushed.statements
    );
}
