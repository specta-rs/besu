//! Schema fidelity on SQLite: the objects the IR cannot express are skipped by
//! introspection, and a declared schema that matches the database converges.
#![cfg(feature = "sqlx")]
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::prelude::*;
use besu::schema::{Check, Index, IndexColumn, IndexKind, Schema};
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

/// A table with both check spellings: an auto-named one and an explicitly named one.
#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "products")]
#[besu(check = "price > 0")]
#[besu(check(
    name = "products_ends_after_it_starts",
    expression = "ends_at IS NULL OR ends_at > starts_at"
))]
#[allow(dead_code, clippy::duplicated_attributes)]
pub struct Product {
    #[besu(primary_key)]
    pub id: i64,
    pub price: i64,
    pub starts_at: chrono::NaiveDateTime,
    pub ends_at: Option<chrono::NaiveDateTime>,
}

#[test]
fn derived_checks_carry_their_names_and_expressions() {
    let schema = Schema::from_defs(&[products.def()]);
    let checks = &schema.table("products").unwrap().checks;

    assert_eq!(checks.len(), 2);
    // The auto name is the table plus the check's zero-based position on the struct.
    assert_eq!(checks[0].name, "products_0_check");
    assert_eq!(checks[0].expression, "price > 0");
    // An explicit name is kept as written.
    assert_eq!(checks[1].name, "products_ends_after_it_starts");
    assert_eq!(
        checks[1].expression,
        "ends_at IS NULL OR ends_at > starts_at"
    );
}

/// Every table-level index spelling: composite, expression, named, unique, an access method
/// and both operator-class forms.
#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "memberships")]
#[besu(index(columns(user_id, team_id)))]
#[besu(index(
    columns(email, lower(email)),
    name = "memberships_email_lower",
    unique,
    using = "gin",
    opclass(email = "text_pattern_ops")
))]
#[besu(index(
    columns(email),
    name = "memberships_email_pattern",
    opclass = "text_pattern_ops"
))]
#[besu(index(columns(lower(email))))]
#[allow(dead_code, clippy::duplicated_attributes)]
pub struct Membership {
    #[besu(primary_key)]
    pub user_id: i64,
    pub team_id: i64,
    pub email: String,
}

#[test]
fn derived_indexes_carry_composite_expression_and_opclass_entries() {
    let schema = Schema::from_defs(&[memberships.def()]);
    let table = schema.table("memberships").unwrap();

    let find = |name: &str| {
        table
            .indexes
            .iter()
            .find(|index| index.name == name)
            .unwrap_or_else(|| panic!("no index named {name}"))
    };

    let composite = find("memberships_user_id_team_id_idx");
    assert_eq!(
        composite.columns,
        [
            IndexColumn::column("user_id"),
            IndexColumn::column("team_id")
        ]
    );
    assert!(!composite.unique);
    assert_eq!(composite.kind, IndexKind::Btree);

    let named = find("memberships_email_lower");
    assert_eq!(
        named.columns,
        [
            IndexColumn::column("email").with_opclass("text_pattern_ops"),
            IndexColumn::expression("lower(email)"),
        ]
    );
    assert!(named.unique);
    assert_eq!(named.kind, IndexKind::Gin);

    let bare = find("memberships_email_pattern");
    assert_eq!(
        bare.columns,
        [IndexColumn::column("email").with_opclass("text_pattern_ops")]
    );

    // An expression that is not named contributes its position, not its text.
    let expression = find("memberships_expr1_idx");
    assert_eq!(
        expression.columns,
        [IndexColumn::expression("lower(email)")]
    );
}

/// A partial index is skipped, because the IR cannot express its predicate. An expression
/// index is read back from the `CREATE INDEX` text in `sqlite_master`, so it round-trips.
#[tokio::test]
async fn a_partial_index_is_skipped_and_an_expression_index_round_trips() {
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
    let events_ir = introspected.table("events").unwrap();
    assert_eq!(
        events_ir.indexes,
        [Index {
            name: "events_lower_payload_idx".into(),
            columns: vec![IndexColumn::expression("lower(payload)")],
            unique: false,
            kind: IndexKind::Btree,
            predicate: None,
        }],
        "the expression index is modelled and the partial one is skipped"
    );

    // Pushing the declaration that matches what is there is a no-op, so nothing is dropped.
    let mut desired = Schema::new();
    desired.push_def(events.def());
    desired.tables[0].indexes = events_ir.indexes.clone();
    let pushed = db.push_schema(&desired).await.unwrap();
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

/// A composite index including an expression is created, read back and converges.
#[tokio::test]
async fn a_composite_expression_index_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let db = database(&dir).await;

    let mut desired = Schema::new();
    desired.push_def(events.def());
    desired.tables[0].indexes.push(Index {
        name: "events_kind_payload_idx".into(),
        columns: vec![
            IndexColumn::column("kind"),
            IndexColumn::expression("lower(payload)"),
        ],
        unique: false,
        kind: IndexKind::Btree,
        predicate: None,
    });

    let first = db.push_schema(&desired).await.unwrap();
    assert_eq!(
        first.statements,
        [
            r#"CREATE TABLE "events" ("id" INTEGER PRIMARY KEY, "kind" TEXT NOT NULL, "payload" TEXT NOT NULL)"#,
            r#"CREATE INDEX "events_kind_payload_idx" ON "events" ("kind", lower(payload))"#,
        ]
    );

    let introspected = db.introspect().await.unwrap();
    assert_eq!(
        introspected.table("events").unwrap().indexes,
        desired.tables[0].indexes
    );
    assert!(db.push_schema(&desired).await.unwrap().is_empty());
}

/// A `CHECK` constraint exists only inside the `CREATE TABLE` text on SQLite, so it is read
/// from there, named the same way the derive names an unnamed one, and a change to it rebuilds
/// the table through the ordinary alter machinery.
#[tokio::test]
async fn a_check_constraint_round_trips() {
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
    assert_eq!(
        introspected.table("accounts").unwrap().checks,
        [Check {
            name: "accounts_0_check".into(),
            expression: "balance>=0".into(),
        }]
    );

    // A snapshot that carries that check is already what the database has.
    let pushed = db.push_schema(&introspected).await.unwrap();
    assert!(pushed.is_empty(), "got {:?}", pushed.changes);

    // The declaration has no check, so push sees a constraint to remove. Dropping one is
    // destructive, and it is applied by rebuilding the table because SQLite has no
    // `ALTER TABLE ... DROP CONSTRAINT`.
    let refused = db.push([accounts.def()]).await.unwrap_err().to_string();
    assert!(refused.contains("accounts_0_check"), "got {refused}");

    let dropped = db.push_destructive([accounts.def()]).await.unwrap();
    assert!(
        dropped
            .statements
            .iter()
            .any(|sql| sql.contains("accounts__besu_tmp")),
        "the change must rebuild the table: {:?}",
        dropped.statements
    );
    assert!(
        db.introspect()
            .await
            .unwrap()
            .table("accounts")
            .unwrap()
            .checks
            .is_empty()
    );
    assert!(db.push([accounts.def()]).await.unwrap().is_empty());
}

/// A check declared by the derive is written into `CREATE TABLE`, read back under the same
/// name and expression, and a second push is a no-op.
#[tokio::test]
async fn a_derived_check_is_created_and_converges() {
    let dir = tempfile::tempdir().unwrap();
    let db = database(&dir).await;

    let first = db.push([products.def()]).await.unwrap();
    assert!(
        first.statements[0].contains(r#"CONSTRAINT "products_0_check" CHECK (price > 0)"#),
        "got {:?}",
        first.statements
    );
    assert!(
        first.statements[0].contains(
            r#"CONSTRAINT "products_ends_after_it_starts" CHECK (ends_at IS NULL OR ends_at > starts_at)"#
        ),
        "got {:?}",
        first.statements
    );

    let introspected = db.introspect().await.unwrap();
    let desired = Schema::from_defs(&[products.def()]);
    let introspected = &introspected.table("products").unwrap().checks;
    let desired = &desired.table("products").unwrap().checks;
    assert_eq!(introspected.len(), desired.len());
    for (introspected, desired) in introspected.iter().zip(desired) {
        assert_eq!(introspected.name, desired.name);
        assert_eq!(
            besu::schema::normalise_expression(&introspected.expression),
            besu::schema::normalise_expression(&desired.expression)
        );
    }
    assert!(db.push([products.def()]).await.unwrap().is_empty());
}

/// A declared check whose expression changed is dropped and added; on SQLite that is one
/// rebuild, and the new expression is what introspection reports afterwards.
#[tokio::test]
async fn a_changed_derived_check_rebuilds_the_table() {
    let dir = tempfile::tempdir().unwrap();
    let db = database(&dir).await;

    db.execute(sql!(
        "CREATE TABLE accounts (\
             id INTEGER PRIMARY KEY, \
             balance INTEGER NOT NULL, \
             CONSTRAINT accounts_balance_check CHECK (balance >= 0))"
    ))
    .await
    .unwrap();

    let mut desired = Schema::new();
    desired.push_def(accounts.def());
    desired.tables[0].checks.push(Check {
        name: "accounts_balance_check".into(),
        expression: "balance>0".into(),
    });

    let changed = db.push_schema_destructive(&desired).await.unwrap();
    assert!(
        changed
            .statements
            .iter()
            .any(|sql| sql.contains("accounts__besu_tmp")),
        "a changed check must rebuild the table: {:?}",
        changed.statements
    );
    assert_eq!(
        db.introspect()
            .await
            .unwrap()
            .table("accounts")
            .unwrap()
            .checks,
        [Check {
            name: "accounts_balance_check".into(),
            expression: "balance>0".into(),
        }]
    );
    assert!(db.push_schema(&desired).await.unwrap().is_empty());
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

/// A rebuild turns foreign keys off for the length of the statement list and back on at its
/// end. A statement that fails in between used to hand the pooled connection back with
/// enforcement still off, so every later statement silently accepted a violation.
#[tokio::test]
async fn a_failed_rebuild_restores_foreign_keys() {
    // One connection, so the statement list that fails and the `PRAGMA` that reads the
    // setting afterwards use the same one.
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.execute(sql!("CREATE TABLE parent (id INTEGER PRIMARY KEY)"))
        .await
        .unwrap();
    db.execute(sql!(
        "CREATE TABLE child (id INTEGER PRIMARY KEY, parent_id INTEGER NOT NULL REFERENCES parent (id))"
    ))
    .await
    .unwrap();
    db.execute(sql!(
        "CREATE TABLE people (id INTEGER PRIMARY KEY, nickname TEXT)"
    ))
    .await
    .unwrap();
    db.execute(sql!("INSERT INTO people (id, nickname) VALUES (1, 'Ada')"))
        .await
        .unwrap();

    // A rebuild (the nullable change) plus a new `NOT NULL` column with no default: the copy
    // has no value for the column, so the rebuild fails after it created its scratch table.
    let current: Schema = db.introspect().await.unwrap();
    let mut doomed = current.clone();
    let table = doomed.table_mut("people").unwrap();
    table.columns[1].nullable = false;
    let mut required = table.columns[0].clone();
    required.name = "required".to_owned();
    required.nullable = false;
    required.primary_key = false;
    required.auto_increment = false;
    table.columns.push(required);

    assert!(db.push_schema(&doomed).await.is_err());

    let enabled: Vec<(i64,)> = db.execute(sql!("PRAGMA foreign_keys")).all().await.unwrap();
    assert_eq!(enabled, [(1,)], "a failed rebuild must restore the pragma");

    let violation = db
        .execute(sql!("INSERT INTO child (id, parent_id) VALUES (9, 999)"))
        .await;
    assert!(violation.is_err(), "foreign keys must still be enforced");

    // The scratch table is not reported while the table it copies still exists, so a fixed
    // diff can be pushed without dropping it by hand.
    let introspected: Schema = db.introspect().await.unwrap();
    assert!(introspected.table("people__besu_tmp").is_none());
    let mut fixed = current.clone();
    fixed.table_mut("people").unwrap().columns[1].nullable = false;
    db.push_schema(&fixed).await.unwrap();
}
