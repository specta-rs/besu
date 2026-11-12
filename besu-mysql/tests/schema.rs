//! Schema fidelity on MySQL: a functional index is skipped rather than reported as a
//! smaller index, access methods round-trip, generated columns converge, and views are
//! created, replaced and dropped.
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::TypeDef;
use besu::prelude::*;
use besu::schema::{Change, Column, Index, IndexKind, Schema, Table as TableIr, View};
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

/// A functional index has an expression where a column name would be. The IR cannot express
/// one, and reporting the remaining key parts would describe an index that does not exist,
/// so the whole index is skipped.
#[tokio::test]
async fn a_functional_index_is_skipped() {
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
        1,
        "the functional index must be skipped as a whole: {:?}",
        events.indexes
    );
    assert_eq!(events.indexes[0].name, "events_kind_idx");
    assert_eq!(events.indexes[0].columns, ["kind"]);

    // Pushing what introspection saw is a no-op: nothing is dropped or re-created.
    assert!(db.push_schema(&introspected).await.unwrap().is_empty());
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
        rls: false,
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
}
