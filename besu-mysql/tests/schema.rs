//! Schema fidelity on MySQL: functional and composite indexes round-trip, access methods
//! round-trip, generated columns converge, and views are created, replaced and dropped.
#![cfg(feature = "sqlx")]
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::TypeDef;
use besu::prelude::*;
use besu::schema::{Change, Column, Index, IndexColumn, IndexKind, Schema, Table as TableIr, View};
use besu_mysql::Sqlx;
use sqlx::MySqlPool;

/// The MySQL server the suite runs against.
const SERVER: &str = "mysql://root:besu@localhost:53306";

/// Create an empty database named `name`, replacing any database left over from a previous
/// run.
async fn fresh(name: &str) -> besu::Database<Sqlx> {
    let admin = MySqlPool::connect(SERVER).await.unwrap();
    sqlx::query(&format!("DROP DATABASE IF EXISTS `{name}`"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("CREATE DATABASE `{name}`"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;

    let url = format!("{SERVER}/{name}");
    besu::Database::new(Sqlx::new(&url).await.unwrap())
}

/// A functional index has an expression where a column name would be. MySQL reports it in
/// `STATISTICS.EXPRESSION`, with its own backticked spelling, which is compared after
/// folding identifier quoting.
#[tokio::test]
async fn a_functional_index_round_trips() {
    let db = fresh("besu_test_schema_functional_index").await;

    db.execute(sql!(
        "CREATE TABLE events (\
             id BIGINT PRIMARY KEY, \
             kind VARCHAR(50) NOT NULL, \
             payload VARCHAR(200) NOT NULL)"
    ))
    .await
    .unwrap();
    db.execute(sql!("CREATE INDEX events_kind_idx ON events (kind)"))
        .await
        .unwrap();
    db.execute(sql!(
        "CREATE INDEX events_lower_payload_idx ON events ((lower(payload)))"
    ))
    .await
    .unwrap();

    let introspected = db.introspect().await.unwrap();
    let events = introspected.table("events").unwrap();
    assert_eq!(
        events.indexes.len(),
        2,
        "every index must be reported: {:?}",
        events.indexes
    );
    let kind = events
        .indexes
        .iter()
        .find(|index| index.name == "events_kind_idx")
        .expect("the plain index must be reported");
    assert_eq!(kind.columns, [IndexColumn::column("kind")]);
    let functional = events
        .indexes
        .iter()
        .find(|index| index.name == "events_lower_payload_idx")
        .expect("the functional index must be reported");
    assert_eq!(
        besu::schema::normalise_generated_expression(
            functional.columns[0]
                .as_expression()
                .expect("a functional key part must be an expression")
        ),
        besu::schema::normalise_generated_expression("lower(payload)")
    );

    // Pushing what introspection saw is a no-op: nothing is dropped or re-created.
    assert!(db.push_schema(&introspected).await.unwrap().is_empty());
}

/// A composite index with a functional key part is created inside `CREATE TABLE`, read back
/// from `STATISTICS` and converges.
#[tokio::test]
async fn a_composite_functional_index_round_trips() {
    let db = fresh("besu_test_schema_composite_functional").await;

    let mut desired = Schema::new();
    desired.tables.push(table(
        "events",
        vec![
            {
                let mut id = column("id", TypeDef::Int8);
                id.primary_key = true;
                id
            },
            column("kind", TypeDef::VarChar(50)),
            column("payload", TypeDef::VarChar(200)),
        ],
    ));
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
            "CREATE TABLE `events` (`id` BIGINT PRIMARY KEY, `kind` VARCHAR(50) NOT NULL, \
             `payload` VARCHAR(200) NOT NULL, \
             KEY `events_kind_payload_idx` (`kind`, (lower(payload)))) \
             ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci",
        ]
    );

    let introspected = db.introspect().await.unwrap();
    let index = &introspected.table("events").unwrap().indexes[0];
    assert_eq!(index.name, "events_kind_payload_idx");
    assert_eq!(index.columns[0], IndexColumn::column("kind"));
    assert_eq!(
        besu::schema::normalise_generated_expression(index.columns[1].as_expression().unwrap()),
        besu::schema::normalise_generated_expression("lower(payload)")
    );
    assert!(db.push_schema(&desired).await.unwrap().is_empty());
}

/// A view is created, replaced with `CREATE OR REPLACE` and dropped. MySQL qualifies and
/// backticks the definition it reports, which must not look like a change.
#[tokio::test]
async fn views_are_created_replaced_and_dropped() {
    let db = fresh("besu_test_schema_view").await;

    let mut desired = Schema::new();
    desired.push_def(users.def());
    desired.add_view(View::new("user_ids", "SELECT id FROM users"));

    let first = db.push_schema(&desired).await.unwrap();
    assert_eq!(
        first.statements,
        [
            "CREATE TABLE `users` (`id` BIGINT PRIMARY KEY) \
             ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci",
            "CREATE VIEW `user_ids` AS SELECT id FROM users",
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
        introspected.views[0].definition.contains("users"),
        "got {:?}",
        introspected.views[0].definition
    );

    // MySQL writes ``select `users`.`id` AS `id` from `users` ``, which is the same view.
    assert!(db.push_schema(&desired).await.unwrap().is_empty());
    assert!(db.push_schema(&introspected).await.unwrap().is_empty());

    let mut changed = desired.clone();
    changed.views[0].definition = "SELECT id FROM users WHERE id > 0".into();
    let replaced = db.push_schema(&changed).await.unwrap();
    assert_eq!(
        replaced.changes,
        [Change::ReplaceView(changed.views[0].clone())]
    );
    assert_eq!(
        replaced.statements,
        ["CREATE OR REPLACE VIEW `user_ids` AS SELECT id FROM users WHERE id > 0"]
    );
    assert!(db.push_schema(&changed).await.unwrap().is_empty());

    let dropped = db.push_schema_destructive(&Schema::new()).await.unwrap();
    assert_eq!(
        dropped.statements,
        ["DROP VIEW `user_ids`", "DROP TABLE `users`"]
    );
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "users")]
#[allow(dead_code)]
pub struct User {
    #[besu(primary_key)]
    pub id: i64,
}

fn column(name: &str, ty: TypeDef) -> Column {
    Column {
        name: name.into(),
        ty,
        nullable: false,
        auto_increment: false,
        primary_key: false,
        unique: false,
        default: None,
        generated: None,
        comment: None,
        renamed_from: None,
    }
}

fn table(name: &str, columns: Vec<Column>) -> TableIr {
    TableIr {
        name: name.into(),
        schema: None,
        columns,
        indexes: Vec::new(),
        uniques: Vec::new(),
        foreign_keys: Vec::new(),
        checks: Vec::new(),
        comment: None,
    }
}

/// `FULLTEXT` and `SPATIAL` are MySQL's own access methods. They are rendered inside
/// `CREATE TABLE` and as `CREATE FULLTEXT INDEX` when the table already exists, and read
/// back from `STATISTICS.INDEX_TYPE`.
#[tokio::test]
async fn fulltext_indexes_round_trip() {
    let db = fresh("besu_test_schema_fulltext").await;

    let base = |kind: Option<IndexKind>| {
        let mut posts = table(
            "posts",
            vec![
                {
                    let mut id = column("id", TypeDef::Int8);
                    id.primary_key = true;
                    id
                },
                column("title", TypeDef::Text),
            ],
        );
        if let Some(kind) = kind {
            posts.indexes.push(Index {
                name: "posts_title_idx".into(),
                columns: vec!["title".into()],
                unique: false,
                kind,
                predicate: None,
            });
        }
        let mut schema = Schema::new();
        schema.tables.push(posts);
        schema.sort();
        schema
    };

    let first = db
        .push_schema(&base(Some(IndexKind::FullText)))
        .await
        .unwrap();
    assert_eq!(
        first.statements,
        [
            "CREATE TABLE `posts` (`id` BIGINT PRIMARY KEY, `title` TEXT NOT NULL, \
          FULLTEXT KEY `posts_title_idx` (`title`)) \
          ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci"
        ]
    );
    assert_eq!(
        db.introspect()
            .await
            .unwrap()
            .table("posts")
            .unwrap()
            .indexes[0]
            .kind,
        IndexKind::FullText
    );
    assert!(
        db.push_schema(&base(Some(IndexKind::FullText)))
            .await
            .unwrap()
            .is_empty()
    );

    // Removing it is destructive, as any dropped index is.
    let dropped = db.push_schema_destructive(&base(None)).await.unwrap();
    assert_eq!(
        dropped.statements,
        ["DROP INDEX `posts_title_idx` ON `posts`"]
    );

    // Adding it back to an existing table uses the standalone spelling.
    let added = db
        .push_schema(&base(Some(IndexKind::FullText)))
        .await
        .unwrap();
    assert_eq!(
        added.statements,
        ["CREATE FULLTEXT INDEX `posts_title_idx` ON `posts` (`title`)"]
    );
    assert!(
        db.push_schema(&base(Some(IndexKind::FullText)))
            .await
            .unwrap()
            .is_empty()
    );
}

/// Adding an index to an existing `TEXT` column needs a key prefix, which only the table's
/// column types can supply. Without it MySQL fails with error 1170.
#[tokio::test]
async fn an_index_added_to_a_text_column_gets_a_prefix() {
    let db = fresh("besu_test_schema_prefix").await;

    let base = |indexed: bool| {
        let mut definition = table(
            "users",
            vec![
                {
                    let mut id = column("id", TypeDef::Int8);
                    id.primary_key = true;
                    id
                },
                column("email", TypeDef::Text),
            ],
        );
        if indexed {
            definition.indexes.push(Index {
                name: "users_email_idx".into(),
                columns: vec!["email".into()],
                unique: false,
                kind: IndexKind::Btree,
                predicate: None,
            });
        }
        let mut schema = Schema::new();
        schema.tables.push(definition);
        schema.sort();
        schema
    };

    db.push_schema(&base(false)).await.unwrap();
    let added = db.push_schema(&base(true)).await.unwrap();
    assert_eq!(
        added.statements,
        ["CREATE INDEX `users_email_idx` ON `users` (`email`(255))"]
    );
    assert!(db.push_schema(&base(true)).await.unwrap().is_empty());
}

/// A `VARCHAR(1000)` cannot be a full unique key in `utf8mb4`: InnoDB's limit is 3072 bytes.
/// The prefix is what makes the statement acceptable, at the documented cost that
/// uniqueness covers only the indexed prefix.
#[tokio::test]
async fn a_wide_varchar_unique_gets_a_prefix() {
    let db = fresh("besu_test_schema_wide_unique").await;

    let users_table = table(
        "users",
        vec![
            {
                let mut id = column("id", TypeDef::Int8);
                id.primary_key = true;
                id
            },
            {
                let mut email = column("email", TypeDef::VarChar(1000));
                email.unique = true;
                email
            },
        ],
    );
    let mut desired = Schema::new();
    desired.tables.push(users_table);
    desired.sort();

    let first = db.push_schema(&desired).await.unwrap();
    assert!(
        first.statements[0].contains("UNIQUE KEY `email` (`email`(768))"),
        "got {:?}",
        first.statements
    );
    assert!(
        db.introspect()
            .await
            .unwrap()
            .table("users")
            .unwrap()
            .column("email")
            .unwrap()
            .unique
    );
    assert!(db.push_schema(&desired).await.unwrap().is_empty());
}

mod generated {
    #![allow(unused_imports)]
    use besu::Table;

    /// A generated column whose expression the database computes.
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

mod checked {
    #![allow(unused_imports)]
    use besu::Table;

    /// A table with both check spellings. MySQL lowercases keywords, backticks identifiers
    /// and escapes string literals in `CHECK_CLAUSE`; all of it has to fold back to this.
    #[derive(Table, Debug, Clone, PartialEq)]
    #[besu(table = "invoices")]
    #[besu(check = "total > 0")]
    #[besu(check(
        name = "invoices_due_after_issued",
        expression = "due_at IS NULL OR due_at > issued_at"
    ))]
    #[besu(check(
        name = "invoices_status",
        expression = "status <> 'void' AND total < 1000000"
    ))]
    #[allow(dead_code, clippy::duplicated_attributes)]
    pub struct Invoice {
        #[besu(primary_key)]
        pub id: i64,
        pub total: i64,
        pub issued_at: chrono::NaiveDateTime,
        pub due_at: Option<chrono::NaiveDateTime>,
        pub status: String,
    }
}

/// A generated column is rendered with `GENERATED ALWAYS AS ... STORED`, read back from
/// `GENERATION_EXPRESSION` and never written by an insert.
#[tokio::test]
async fn generated_columns_round_trip() {
    let db = fresh("besu_test_schema_generated").await;

    let first = db.push([generated::line_items.def()]).await.unwrap();
    assert!(
        first.statements[0]
            .contains("`total` BIGINT GENERATED ALWAYS AS (price * quantity) STORED NOT NULL"),
        "got {:?}",
        first.statements
    );

    let introspected = db.introspect().await.unwrap();
    let total = introspected
        .table("line_items")
        .unwrap()
        .column("total")
        .unwrap();
    assert_eq!(
        total.generated,
        Some(besu::schema::Generated {
            expression: "price*quantity".into(),
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
}

/// A check declared by the derive is written into `CREATE TABLE`, read back from
/// `information_schema.CHECK_CONSTRAINTS`, and converges despite MySQL's backticks,
/// lowercased keywords and escaped string literals.
#[tokio::test]
async fn derived_checks_are_created_and_converge() {
    let db = fresh("besu_test_schema_checks").await;

    let first = db.push([checked::invoices.def()]).await.unwrap();
    assert!(
        first.statements[0].contains("CONSTRAINT `invoices_0_check` CHECK (total > 0)"),
        "got {:?}",
        first.statements
    );
    assert!(
        first.statements[0].contains(
            "CONSTRAINT `invoices_due_after_issued` CHECK (due_at IS NULL OR due_at > issued_at)"
        ),
        "got {:?}",
        first.statements
    );
    assert!(
        first.statements[0]
            .contains("CONSTRAINT `invoices_status` CHECK (status <> 'void' AND total < 1000000)"),
        "got {:?}",
        first.statements
    );

    let introspected = db.introspect().await.unwrap();
    let desired = besu::Database::<Sqlx>::schema([checked::invoices.def()]);
    let introspected = &introspected.table("invoices").unwrap().checks;
    let desired = &desired.table("invoices").unwrap().checks;
    assert_eq!(introspected.len(), desired.len());
    for (introspected, desired) in introspected.iter().zip(desired) {
        assert_eq!(introspected.name, desired.name);
        assert_eq!(
            besu::schema::normalise_expression(&introspected.expression),
            besu::schema::normalise_expression(&desired.expression)
        );
    }
    assert!(db.push([checked::invoices.def()]).await.unwrap().is_empty());
}

/// A check can be added to and dropped from an existing table, and MySQL's drop spelling is
/// `DROP CHECK` (8.0.16) rather than `DROP CONSTRAINT` (8.0.19).
#[tokio::test]
async fn a_check_is_added_and_dropped_on_an_existing_table() {
    let db = fresh("besu_test_schema_check_alter").await;

    db.execute(sql!(
        "CREATE TABLE accounts (\
             id BIGINT PRIMARY KEY, \
             balance BIGINT NOT NULL)"
    ))
    .await
    .unwrap();

    let base = || {
        let mut schema = Schema::new();
        schema.tables.push(table(
            "accounts",
            vec![
                {
                    let mut id = column("id", TypeDef::Int8);
                    id.primary_key = true;
                    id
                },
                column("balance", TypeDef::Int8),
            ],
        ));
        schema.sort();
        schema
    };

    let mut with = base();
    with.tables[0].checks.push(besu::schema::Check {
        name: "accounts_balance_check".into(),
        expression: "balance>=0".into(),
    });

    let added = db.push_schema(&with).await.unwrap();
    assert_eq!(
        added.statements,
        ["ALTER TABLE `accounts` ADD CONSTRAINT `accounts_balance_check` CHECK (balance>=0)"]
    );
    assert!(db.push_schema(&with).await.unwrap().is_empty());

    let dropped = db.push_schema_destructive(&base()).await.unwrap();
    assert_eq!(
        dropped.statements,
        ["ALTER TABLE `accounts` DROP CHECK `accounts_balance_check`"]
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
    assert!(db.push_schema(&base()).await.unwrap().is_empty());
}

// ---------------------------------------------------------------------------------------
// Introspection fidelity
// ---------------------------------------------------------------------------------------

/// A table whose columns use the native spellings `TEXT`, `INT` and `BIGINT` would flatten,
/// plus the modifiers `EXTRA` and `COLLATION_NAME` carry.
#[tokio::test]
async fn native_type_names_and_modifiers_are_kept() {
    let db = fresh("besu_test_fidelity").await;
    db.execute(sql!(
        "CREATE TABLE fidelity (\
             id BIGINT PRIMARY KEY, \
             tiny TINYINT NOT NULL, \
             medium MEDIUMINT NOT NULL, \
             yr YEAR NOT NULL, \
             body LONGTEXT NOT NULL, \
             blob_data LONGBLOB NOT NULL, \
             at TIME(6) NOT NULL, \
             stamp DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6), \
             name VARCHAR(20) COLLATE utf8mb4_bin NOT NULL)"
    ))
    .await
    .unwrap();

    let introspected = db.introspect().await.unwrap();
    let table = introspected.table("fidelity").unwrap();
    assert_eq!(
        table.column("tiny").unwrap().ty,
        TypeDef::Custom("tinyint".into())
    );
    assert_eq!(
        table.column("medium").unwrap().ty,
        TypeDef::Custom("mediumint".into())
    );
    assert_eq!(
        table.column("yr").unwrap().ty,
        TypeDef::Custom("year".into())
    );
    assert_eq!(
        table.column("body").unwrap().ty,
        TypeDef::Custom("longtext".into())
    );
    assert_eq!(
        table.column("blob_data").unwrap().ty,
        TypeDef::Custom("longblob".into())
    );
    assert_eq!(table.column("at").unwrap().ty, TypeDef::TimeP(6));
    assert_eq!(
        table.column("name").unwrap().ty,
        TypeDef::Collated(Box::new(TypeDef::VarChar(20)), "utf8mb4_bin".into())
    );
    let default = table.column("stamp").unwrap().default.as_deref().unwrap();
    assert!(
        default.contains("ON UPDATE CURRENT_TIMESTAMP(6)"),
        "got {default:?}"
    );

    // The snapshot replays to the same schema: nothing is altered by pushing it back.
    let pushed = db.push_schema(&introspected).await.unwrap();
    assert!(pushed.is_empty(), "got {:?}", pushed.changes);
}

/// Replaying the snapshot into an empty database reproduces the declared types.
#[tokio::test]
async fn a_snapshot_replays_to_the_same_types() {
    let source = fresh("besu_test_fidelity_source").await;
    source
        .execute(sql!(
            "CREATE TABLE replay (\
                 id BIGINT PRIMARY KEY, \
                 tiny TINYINT NOT NULL, \
                 body LONGTEXT NOT NULL, \
                 at TIME(3) NOT NULL, \
                 name VARCHAR(20) COLLATE utf8mb4_bin NOT NULL)"
        ))
        .await
        .unwrap();
    let snapshot = source.introspect().await.unwrap();

    let destination = fresh("besu_test_fidelity_destination").await;
    destination.push_schema(&snapshot).await.unwrap();

    let replayed = destination.introspect().await.unwrap();
    let replayed = replayed.table("replay").unwrap();
    assert_eq!(
        replayed.column("tiny").unwrap().ty,
        TypeDef::Custom("tinyint".into())
    );
    assert_eq!(
        replayed.column("body").unwrap().ty,
        TypeDef::Custom("longtext".into())
    );
    assert_eq!(replayed.column("at").unwrap().ty, TypeDef::TimeP(3));
    assert_eq!(
        replayed.column("name").unwrap().ty,
        TypeDef::Collated(Box::new(TypeDef::VarChar(20)), "utf8mb4_bin".into())
    );
}

/// A declared `NaiveTime` renders `TIME(6)` and comes back with its precision folded, so a
/// second push is still a no-op.
#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "clocks")]
pub struct Clock {
    #[besu(primary_key)]
    pub id: i64,
    pub at: chrono::NaiveTime,
}

#[tokio::test]
async fn a_time_keeps_its_nanoseconds() {
    let db = fresh("besu_test_time_precision").await;
    let first = db.push([clocks.def()]).await.unwrap();
    assert!(
        first.statements[0].contains("`at` TIME(6) NOT NULL"),
        "{:?}",
        first.statements
    );
    let second = db.push([clocks.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);

    let value = chrono::NaiveTime::from_hms_nano_opt(1, 2, 3, 456_789_000).unwrap();
    db.insert(clocks)
        .value(clocks.insert(1, value))
        .await
        .unwrap();
    let stored: Vec<Clock> = db.select(clocks).await.unwrap();
    assert_eq!(stored[0].at, value);
}
