//! Schema fidelity on SQLite: the objects the IR cannot express are skipped by
//! introspection, and a declared schema that matches the database converges.
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::prelude::*;
use besu::schema::Schema;
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

/// A view is created, replaced by a drop and a create (SQLite has no `OR REPLACE`) and
/// dropped before the table it reads. Introspection reports the whole `CREATE VIEW`
/// statement, which is compared by its body.
#[tokio::test]
async fn views_are_created_replaced_and_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let db = database(&dir).await;

    let mut desired = Schema::new();
    desired.push_def(users.def());
    desired.add_view(besu::schema::View::new("user_ids", "SELECT id FROM users"));

    let first = db.push_schema(&desired).await.unwrap();
    assert_eq!(
        first.statements,
        [
            r#"CREATE TABLE "users" ("id" INTEGER PRIMARY KEY)"#,
            r#"CREATE VIEW "user_ids" AS SELECT id FROM users"#,
        ]
    );

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

    // The recorded definition includes the `CREATE VIEW` header, which is not a change.
    assert!(db.push_schema(&desired).await.unwrap().is_empty());
    assert!(db.push_schema(&introspected).await.unwrap().is_empty());

    let mut changed = desired.clone();
    changed.views[0].definition = "SELECT id FROM users WHERE id > 0".into();
    let replaced = db.push_schema(&changed).await.unwrap();
    assert_eq!(
        replaced.statements,
        [
            r#"DROP VIEW IF EXISTS "user_ids""#,
            r#"CREATE VIEW "user_ids" AS SELECT id FROM users WHERE id > 0"#,
        ]
    );
    assert!(db.push_schema(&changed).await.unwrap().is_empty());

    let dropped = db.push_schema_destructive(&Schema::new()).await.unwrap();
    assert_eq!(
        dropped.statements,
        [r#"DROP VIEW "user_ids""#, r#"DROP TABLE "users""#]
    );
}

mod generated {
    #![allow(unused_imports)]
    use besu::Table;

    /// A table whose `total` column SQLite computes from the other two.
    #[derive(Table, Debug, Clone, PartialEq)]
    #[besu(table = "line_items")]
    #[allow(dead_code)]
    pub struct LineItem {
        #[besu(primary_key)]
        pub id: i64,
        pub price: i64,
        pub quantity: i64,
        #[besu(generated = "price * quantity")]
        pub total: i64,
    }
}

mod generated_more {
    #![allow(unused_imports)]
    use besu::Table;

    /// The same table with one more generated column, to exercise `ADD COLUMN`.
    #[derive(Table, Debug, Clone, PartialEq)]
    #[besu(table = "line_items")]
    #[allow(dead_code)]
    pub struct LineItem {
        #[besu(primary_key)]
        pub id: i64,
        pub price: i64,
        pub quantity: i64,
        #[besu(generated = "price * quantity")]
        pub total: i64,
        #[besu(generated = "price + quantity", stored = false)]
        pub shift: i64,
    }
}

/// SQLite hides a generated column from `PRAGMA table_info` entirely. `table_xinfo` reports
/// it, and the expression is read out of the `CREATE TABLE` text that SQLite stores.
#[tokio::test]
async fn generated_columns_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let db = database(&dir).await;

    let first = db.push([generated::line_items.def()]).await.unwrap();
    assert_eq!(
        first.statements,
        [
            r#"CREATE TABLE "line_items" ("id" INTEGER PRIMARY KEY, "price" INTEGER NOT NULL, "quantity" INTEGER NOT NULL, "total" INTEGER GENERATED ALWAYS AS (price * quantity) STORED NOT NULL)"#
        ]
    );

    let introspected = db.introspect().await.unwrap();
    assert_eq!(
        introspected
            .table("line_items")
            .unwrap()
            .column("total")
            .unwrap()
            .generated,
        Some(besu::schema::Generated {
            expression: "price * quantity".into(),
            stored: true,
        })
    );
    assert!(
        db.push([generated::line_items.def()])
            .await
            .unwrap()
            .is_empty()
    );

    db.execute(sql!(
        "INSERT INTO line_items (id, price, quantity) VALUES (1, 3, 4)"
    ))
    .await
    .unwrap();
    let totals: Vec<i64> = db
        .execute(sql!("SELECT total FROM line_items"))
        .all()
        .await
        .unwrap();
    assert_eq!(totals, [12]);

    // A second generated column is one `ADD COLUMN`, and the virtual spelling round-trips
    // through `table_xinfo` too.
    let added = db.push([generated_more::line_items.def()]).await.unwrap();
    assert_eq!(
        added.statements,
        [
            r#"ALTER TABLE "line_items" ADD COLUMN "shift" INTEGER GENERATED ALWAYS AS (price + quantity) VIRTUAL NOT NULL"#
        ]
    );
    let introspected = db.introspect().await.unwrap();
    let shift = introspected
        .table("line_items")
        .unwrap()
        .column("shift")
        .unwrap();
    assert_eq!(
        shift.generated,
        Some(besu::schema::Generated {
            expression: "price + quantity".into(),
            stored: false,
        })
    );
    assert!(
        db.push([generated_more::line_items.def()])
            .await
            .unwrap()
            .is_empty()
    );
}

/// SQLite has one access method, so every index it reports is a b-tree and a declared
/// b-tree index converges.
#[tokio::test]
async fn an_index_is_a_b_tree() {
    let dir = tempfile::tempdir().unwrap();
    let db = database(&dir).await;

    let mut desired = Schema::new();
    desired.push_def(events.def());
    desired.tables[0].indexes.push(besu::schema::Index {
        name: "events_kind_idx".into(),
        columns: vec!["kind".into()],
        unique: false,
        kind: besu::schema::IndexKind::Btree,
        predicate: None,
    });

    db.push_schema(&desired).await.unwrap();
    let introspected = db.introspect().await.unwrap();
    assert_eq!(
        introspected.table("events").unwrap().indexes[0].kind,
        besu::schema::IndexKind::Btree
    );
    assert!(db.push_schema(&desired).await.unwrap().is_empty());
}
