#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

//! Schema fidelity: native types, enum types, composite `UNIQUE` constraints,
//! schema-qualified tables, check constraints, views and the index shapes the IR cannot
//! express, all exercised against a real PostgreSQL server.
#![cfg(feature = "sqlx")]

mod common;

use besu::TypeDef;
use besu::prelude::*;
use besu::schema::{
    Change, Check, Column, Enum, Generated, Index, IndexColumn, IndexKind, Safety, Schema,
    Sequence, Table as TableIr, Unique, View,
};
use besu_postgres::Sqlx;

/// A `NOT NULL` column with no constraint of its own.
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

fn schema(tables: Vec<TableIr>, enums: Vec<Enum>) -> Schema {
    let mut schema = Schema {
        tables,
        enums,
        views: Vec::new(),
        ..Schema::default()
    };
    schema.sort();
    schema
}

/// `TIMETZ` is modelled in the IR so it round-trips and renders, even though no driver
/// decodes it.
#[tokio::test]
async fn time_tz_round_trips() {
    let db = common::database().await;
    common::reset(&db).await;

    let desired = schema(
        vec![{
            let mut at = table("appointments", vec![column("at", TypeDef::TimeTz)]);
            at.columns[0].nullable = true;
            at
        }],
        Vec::new(),
    );

    let first = db.push_schema(&desired).await.unwrap();
    assert_eq!(
        first.statements,
        [r#"CREATE TABLE "appointments" ("at" TIMETZ)"#]
    );

    let introspected = db.introspect().await.unwrap();
    assert_eq!(
        introspected
            .table("appointments")
            .unwrap()
            .column("at")
            .unwrap()
            .ty,
        TypeDef::TimeTz
    );

    let second = db.push_schema(&desired).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);
}

/// A composite `UNIQUE` is a constraint, not an index: PostgreSQL owns the index, so
/// `DROP INDEX` cannot remove it and `DROP CONSTRAINT` is the only way to get rid of it.
#[tokio::test]
async fn composite_unique_is_a_constraint() {
    let db = common::database().await;
    common::reset(&db).await;

    db.execute(sql!(
        "CREATE TABLE memberships (\
             user_id BIGINT NOT NULL, \
             team_id BIGINT NOT NULL, \
             CONSTRAINT memberships_user_team_key UNIQUE (user_id, team_id))"
    ))
    .await
    .unwrap();

    let introspected = db.introspect().await.unwrap();
    let memberships = introspected.table("memberships").unwrap();
    assert_eq!(
        memberships.uniques,
        [Unique {
            name: "memberships_user_team_key".into(),
            columns: vec!["user_id".into(), "team_id".into()],
        }]
    );
    assert!(
        memberships.indexes.is_empty(),
        "a constraint-backed index must not be reported: {:?}",
        memberships.indexes
    );

    let with = schema(
        vec![{
            let mut table = table(
                "memberships",
                vec![
                    column("user_id", TypeDef::Int8),
                    column("team_id", TypeDef::Int8),
                ],
            );
            table.uniques.push(Unique {
                name: "memberships_user_team_key".into(),
                columns: vec!["user_id".into(), "team_id".into()],
            });
            table
        }],
        Vec::new(),
    );

    // The declared schema already matches, so nothing happens.
    assert!(db.push_schema(&with).await.unwrap().is_empty());

    // Removing the constraint is destructive, and the statement is one PostgreSQL accepts.
    let without = schema(
        vec![table(
            "memberships",
            vec![
                column("user_id", TypeDef::Int8),
                column("team_id", TypeDef::Int8),
            ],
        )],
        Vec::new(),
    );
    assert!(db.push_schema(&without).await.is_err());
    let dropped = db.push_schema_destructive(&without).await.unwrap();
    assert_eq!(
        dropped.statements,
        [r#"ALTER TABLE "memberships" DROP CONSTRAINT "memberships_user_team_key""#]
    );
    assert!(
        db.introspect()
            .await
            .unwrap()
            .table("memberships")
            .unwrap()
            .uniques
            .is_empty()
    );

    // Adding it back is a plain `ALTER TABLE`, and then the schema converges again.
    let added = db.push_schema(&with).await.unwrap();
    assert_eq!(
        added.statements,
        [
            r#"ALTER TABLE "memberships" ADD CONSTRAINT "memberships_user_team_key" UNIQUE ("user_id", "team_id")"#
        ]
    );
    assert!(db.push_schema(&with).await.unwrap().is_empty());
}

/// Enum types are introspected, diffed, and rendered with `CREATE TYPE` before the table
/// that uses them.
#[tokio::test]
async fn enum_types_are_created_diffed_and_dropped() {
    let db = common::database().await;
    common::reset(&db).await;

    let mut review = table(
        "reviews",
        vec![
            {
                let mut id = column("id", TypeDef::Int8);
                id.primary_key = true;
                id
            },
            column("mood", TypeDef::Custom("mood".into())),
        ],
    );
    review.uniques = Vec::new();

    let desired = schema(
        vec![review.clone()],
        vec![Enum {
            name: "mood".into(),
            values: vec!["sad".into(), "ok".into(), "happy".into()],
        }],
    );

    let first = db.push_schema(&desired).await.unwrap();
    assert_eq!(
        first.statements,
        [
            r#"CREATE TYPE "mood" AS ENUM ('sad', 'ok', 'happy')"#,
            r#"CREATE TABLE "reviews" ("id" BIGINT PRIMARY KEY, "mood" "mood" NOT NULL)"#,
        ]
    );

    let introspected = db.introspect().await.unwrap();
    assert_eq!(
        introspected.enums,
        [Enum {
            name: "mood".into(),
            values: vec!["sad".into(), "ok".into(), "happy".into()],
        }]
    );
    // A column of an enum type is a custom type, not `TEXT`.
    assert_eq!(
        introspected
            .table("reviews")
            .unwrap()
            .column("mood")
            .unwrap()
            .ty,
        TypeDef::Custom("mood".into())
    );

    assert!(db.push_schema(&desired).await.unwrap().is_empty());

    // Appending a value is risky but applies.
    let mut grown = desired.clone();
    grown.enums[0].values.push("ecstatic".into());
    let added = db.push_schema(&grown).await.unwrap();
    assert_eq!(
        added.changes,
        [Change::AddEnumValue {
            name: "mood".into(),
            value: "ecstatic".into(),
        }]
    );
    assert_eq!(added.changes[0].safety(), Safety::Risky);
    assert_eq!(
        added.statements,
        [r#"ALTER TYPE "mood" ADD VALUE 'ecstatic'"#]
    );
    assert!(db.push_schema(&grown).await.unwrap().is_empty());

    // Removing a value would break every row that still holds it, so it is destructive and
    // cannot even be expressed: PostgreSQL has no `ALTER TYPE ... DROP VALUE`.
    let mut shrunk = grown.clone();
    shrunk.enums[0].values.retain(|value| value != "sad");
    let refused = db.push_schema(&shrunk).await.unwrap_err().to_string();
    assert!(
        refused.contains("remove value sad from enum mood"),
        "got {refused}"
    );

    // Dropping the table and then the type, in that order, is destructive too.
    let empty = Schema::new();
    let refused = db.push_schema(&empty).await.unwrap_err().to_string();
    assert!(refused.contains("drop table reviews"), "got {refused}");
    assert!(refused.contains("drop enum mood"), "got {refused}");

    let dropped = db.push_schema_destructive(&empty).await.unwrap();
    assert_eq!(
        dropped.statements,
        [r#"DROP TABLE "reviews""#, r#"DROP TYPE "mood""#]
    );
    assert!(db.push_schema(&empty).await.unwrap().is_empty());
}

#[allow(unused_imports)]
mod scoped {
    use besu::prelude::*;

    /// The same table name as the unscoped one, in a schema of its own.
    #[derive(Table, Debug, Clone, PartialEq)]
    #[besu(table = "things", schema = "besu_schema_test")]
    #[allow(dead_code)]
    pub struct ScopedThing {
        #[besu(primary_key)]
        pub id: i64,
        pub label: String,
    }
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "things")]
#[allow(dead_code)]
pub struct Thing {
    #[besu(primary_key)]
    pub id: i64,
    pub label: String,
}

/// Two tables with the same name in different schemas are two tables.
#[tokio::test]
async fn the_same_table_name_in_two_schemas() {
    let db = common::database().await;
    common::reset(&db).await;
    db.execute(sql!("CREATE SCHEMA IF NOT EXISTS \"besu_schema_test\""))
        .await
        .unwrap();

    let first = db.push([things.def(), scoped::things.def()]).await.unwrap();
    assert_eq!(first.changes.len(), 2);
    assert!(
        first
            .statements
            .iter()
            .any(|sql| sql.starts_with(r#"CREATE TABLE "besu_schema_test"."things""#)),
        "got {:?}",
        first.statements
    );
    assert!(
        first
            .statements
            .iter()
            .any(|sql| sql.starts_with(r#"CREATE TABLE "things""#)),
        "got {:?}",
        first.statements
    );

    let introspected = db.introspect().await.unwrap();
    assert_eq!(
        introspected
            .tables
            .iter()
            .filter(|t| t.name == "things")
            .count(),
        2
    );
    let scoped = introspected
        .find(Some("besu_schema_test"), "things")
        .expect("the scoped table must be reported with its schema");
    assert_eq!(scoped.schema.as_deref(), Some("besu_schema_test"));
    assert!(
        introspected
            .find(None, "things")
            .expect("the default-schema table must be reported")
            .schema
            .is_none()
    );

    // The declared schema and the database agree exactly.
    assert_eq!(
        introspected,
        besu::Database::<Sqlx>::schema([things.def(), scoped::things.def()])
    );

    let second = db.push([things.def(), scoped::things.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);

    // A schema the declared schema never mentions is left alone: the unqualified `things`
    // can be pushed on its own without wanting to drop the scoped one.
    let alone = db.push([things.def()]).await.unwrap();
    assert!(alone.is_empty(), "got {:?}", alone.changes);

    db.execute(sql!("DROP SCHEMA \"besu_schema_test\" CASCADE"))
        .await
        .unwrap();
}

/// A partial index and an expression index are both modelled; an `INCLUDE` index is not.
/// Pushing a schema that matches what is really there must not try to drop any of them.
#[tokio::test]
async fn partial_and_expression_indexes_are_not_dropped() {
    let db = common::database().await;
    common::reset(&db).await;

    db.execute(sql!(
        "CREATE TABLE events (\
             id BIGINT PRIMARY KEY, \
             kind TEXT NOT NULL, \
             payload TEXT NOT NULL)"
    ))
    .await
    .unwrap();
    // Only the error rows are indexed, so the index is smaller than a plain one on `kind`.
    db.execute(sql!(
        "CREATE INDEX events_kind_idx ON events (kind) WHERE kind = 'error'"
    ))
    .await
    .unwrap();
    // An index on an expression has no column list to report.
    db.execute(sql!(
        "CREATE INDEX events_lower_payload_idx ON events (lower(payload))"
    ))
    .await
    .unwrap();
    // A non-key `INCLUDE` column has no spelling in the IR either.
    db.execute(sql!(
        "CREATE INDEX events_kind_include_idx ON events (kind) INCLUDE (payload)"
    ))
    .await
    .unwrap();

    let introspected = db.introspect().await.unwrap();
    let events = introspected.table("events").unwrap();
    assert_eq!(
        events.indexes.len(),
        2,
        "the `INCLUDE` index must not be reported: {:?}",
        events.indexes
    );
    let partial = events
        .indexes
        .iter()
        .find(|index| index.name == "events_kind_idx")
        .expect("the partial index must be reported");
    assert_eq!(partial.columns, [IndexColumn::column("kind")]);
    assert!(!partial.unique);
    let predicate = partial
        .predicate
        .as_deref()
        .expect("a partial index must carry its predicate");
    assert!(predicate.contains("error"), "got {predicate}");
    let expression = events
        .indexes
        .iter()
        .find(|index| index.name == "events_lower_payload_idx")
        .expect("the expression index must be reported");
    assert_eq!(
        expression.columns,
        [IndexColumn::expression("lower(payload)")]
    );

    // The declared schema says exactly what the database has, so there is nothing to do.
    let desired = schema(
        vec![{
            let mut events = table(
                "events",
                vec![
                    {
                        let mut id = column("id", TypeDef::Int8);
                        id.primary_key = true;
                        id
                    },
                    column("kind", TypeDef::Text),
                    column("payload", TypeDef::Text),
                ],
            );
            events.indexes.push(partial.clone());
            events.indexes.push(expression.clone());
            events
        }],
        Vec::new(),
    );

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

    // Pushing the introspected schema itself is also a no-op, views and all.
    assert!(db.push_schema(&introspected).await.unwrap().is_empty());
}

/// A composite index with an operator class, an expression and a predicate is rendered in
/// that order, read back from `pg_index` and converges. Changing the operator class is a
/// drop and a create because both statements use the same name.
#[tokio::test]
async fn composite_expression_and_opclass_indexes_round_trip() {
    let db = common::database().await;
    common::reset(&db).await;

    let mut desired = schema(
        vec![table(
            "users",
            vec![
                {
                    let mut id = column("id", TypeDef::Int8);
                    id.primary_key = true;
                    id
                },
                column("email", TypeDef::Text),
                column("name", TypeDef::Text),
                column("payload", TypeDef::Text),
            ],
        )],
        Vec::new(),
    );
    desired.tables[0].indexes.push(Index {
        name: "users_search_idx".into(),
        columns: vec![
            IndexColumn::column("email").with_opclass("text_pattern_ops"),
            IndexColumn::expression("lower(name)"),
        ],
        unique: false,
        kind: IndexKind::Btree,
        predicate: Some("payload IS NOT NULL".into()),
    });

    let first = db.push_schema(&desired).await.unwrap();
    assert_eq!(
        first.statements,
        [
            r#"CREATE TABLE "users" ("id" BIGINT PRIMARY KEY, "email" TEXT NOT NULL, "name" TEXT NOT NULL, "payload" TEXT NOT NULL)"#,
            r#"CREATE INDEX "users_search_idx" ON "users" ("email" text_pattern_ops, lower(name)) WHERE payload IS NOT NULL"#,
        ]
    );

    let introspected = db.introspect().await.unwrap();
    let index = introspected
        .table("users")
        .unwrap()
        .indexes
        .iter()
        .find(|index| index.name == "users_search_idx")
        .expect("the index must be reported");
    assert_eq!(index.columns, desired.tables[0].indexes[0].columns);
    assert!(db.push_schema(&desired).await.unwrap().is_empty());

    // Removing the operator class changes which predicates the index serves, so it is
    // replaced. The default operator class is not reported back, which is what keeps the
    // plain declaration converging.
    let mut changed = desired.clone();
    changed.tables[0].indexes[0].columns[0] = IndexColumn::column("email");
    let replaced = db.push_schema_destructive(&changed).await.unwrap();
    assert_eq!(
        replaced.statements,
        [
            r#"DROP INDEX "users_search_idx""#,
            r#"CREATE INDEX "users_search_idx" ON "users" ("email", lower(name)) WHERE payload IS NOT NULL"#,
        ]
    );
    assert!(db.push_schema(&changed).await.unwrap().is_empty());
}

/// An exclusion constraint is not modelled. PostgreSQL's `information_schema` does not list
/// it among the constraints, so its backing index has to be recognised by the catalogue or
/// it looks like an ordinary index and `push` tries to drop it.
#[tokio::test]
async fn an_exclusion_constraint_is_not_reported_as_an_index() {
    let db = common::database().await;
    common::reset(&db).await;

    db.execute(sql!(
        "CREATE TABLE bookings (\
             room BIGINT NOT NULL, \
             during TSRANGE NOT NULL, \
             EXCLUDE USING gist (during WITH &&))"
    ))
    .await
    .unwrap();

    let introspected = db.introspect().await.unwrap();
    let bookings = introspected.table("bookings").unwrap();
    assert!(
        bookings.indexes.is_empty(),
        "the exclusion constraint's index must not be reported: {:?}",
        bookings.indexes
    );
    assert!(bookings.uniques.is_empty());

    // Nothing in the IR describes the exclusion constraint, so a push of what introspection
    // saw leaves it alone.
    let pushed = db.push_schema(&introspected).await.unwrap();
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

/// `CHECK` constraints are introspected, add and drop with `ALTER TABLE`, and converge even
/// though PostgreSQL stores them with an extra pair of parentheses.
#[tokio::test]
async fn check_constraints_round_trip() {
    let db = common::database().await;
    common::reset(&db).await;

    db.execute(sql!(
        "CREATE TABLE accounts (\
             id BIGINT PRIMARY KEY, \
             balance BIGINT NOT NULL, \
             CONSTRAINT accounts_balance_check CHECK (balance >= 0))"
    ))
    .await
    .unwrap();

    let introspected = db.introspect().await.unwrap();
    assert_eq!(
        introspected.table("accounts").unwrap().checks,
        [Check {
            name: "accounts_balance_check".into(),
            expression: "balance>=0".into(),
        }]
    );

    let accounts = |checks: Vec<Check>| {
        schema(
            vec![{
                let mut accounts = table(
                    "accounts",
                    vec![
                        {
                            let mut id = column("id", TypeDef::Int8);
                            id.primary_key = true;
                            id
                        },
                        column("balance", TypeDef::Int8),
                    ],
                );
                accounts.checks = checks;
                accounts
            }],
            Vec::new(),
        )
    };
    let check = Check {
        name: "accounts_balance_check".into(),
        expression: "balance >= 0".into(),
    };
    let with = accounts(vec![check.clone()]);
    let without = accounts(Vec::new());

    // The declared spelling has one pair of parentheses fewer and still matches.
    assert!(db.push_schema(&with).await.unwrap().is_empty());

    // Dropping the constraint loses a guarantee, so it is destructive and refused.
    let refused = db.push_schema(&without).await.unwrap_err().to_string();
    assert!(
        refused.contains("drop check accounts_balance_check"),
        "got {refused}"
    );
    let dropped = db.push_schema_destructive(&without).await.unwrap();
    assert_eq!(
        dropped.statements,
        [r#"ALTER TABLE "accounts" DROP CONSTRAINT "accounts_balance_check""#]
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

    // Adding it back is one statement, and then the schema converges again.
    let added = db.push_schema(&with).await.unwrap();
    assert_eq!(
        added.statements,
        [r#"ALTER TABLE "accounts" ADD CONSTRAINT "accounts_balance_check" CHECK (balance >= 0)"#]
    );
    assert_eq!(added.changes[0].safety(), Safety::Risky);
    assert!(db.push_schema(&with).await.unwrap().is_empty());
}

/// A check declared by the derive is written into `CREATE TABLE` under its explicit name,
/// read back with PostgreSQL's extra parentheses and casts folded away, and converges.
#[tokio::test]
async fn derived_checks_are_created_and_converge() {
    let db = common::database().await;
    common::reset(&db).await;

    let first = db.push([checked::invoices.def()]).await.unwrap();
    assert!(
        first.statements[0].contains(r#"CONSTRAINT "invoices_0_check" CHECK (total > 0)"#),
        "got {:?}",
        first.statements
    );
    assert!(
        first.statements[0].contains(
            r#"CONSTRAINT "invoices_due_after_issued" CHECK (due_at IS NULL OR due_at > issued_at)"#
        ),
        "got {:?}",
        first.statements
    );
    assert!(
        first.statements[0].contains(
            r#"CONSTRAINT "invoices_status" CHECK (status <> 'void' AND total < 1000000)"#
        ),
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

/// A view is declared, created after the tables it reads, replaced when its definition
/// changes, and dropped before them. Introspection reports what is really there, so pushing
/// the same schema twice is a no-op even though PostgreSQL rewrites the definition.
#[tokio::test]
async fn views_are_created_replaced_and_dropped() {
    let db = common::database().await;
    common::reset(&db).await;

    let users = || {
        schema(
            vec![table(
                "users",
                vec![{
                    let mut id = column("id", TypeDef::Int8);
                    id.primary_key = true;
                    id
                }],
            )],
            Vec::new(),
        )
    };

    let mut desired = users();
    desired.add_view(View::new("user_ids", "SELECT id FROM users"));

    let first = db.push_schema(&desired).await.unwrap();
    assert_eq!(
        first.statements,
        [
            r#"CREATE TABLE "users" ("id" BIGINT PRIMARY KEY)"#,
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
    assert!(!introspected.views[0].materialised);
    assert!(
        introspected.views[0].definition.contains("users"),
        "got {:?}",
        introspected.views[0].definition
    );

    // PostgreSQL qualifies every column, which must not look like a change.
    assert!(db.push_schema(&desired).await.unwrap().is_empty());
    assert!(db.push_schema(&introspected).await.unwrap().is_empty());

    // A changed definition is replaced, not dropped and created.
    let mut changed = desired.clone();
    changed.views[0].definition = "SELECT id FROM users WHERE id > 0".into();
    let replaced = db.push_schema(&changed).await.unwrap();
    assert_eq!(
        replaced.changes,
        [Change::ReplaceView(changed.views[0].clone())]
    );
    assert_eq!(replaced.changes[0].safety(), Safety::Risky);
    assert_eq!(
        replaced.statements,
        [r#"CREATE OR REPLACE VIEW "user_ids" AS SELECT id FROM users WHERE id > 0"#]
    );
    assert!(db.push_schema(&changed).await.unwrap().is_empty());

    // Dropping the table means dropping the view first.
    let dropped = db.push_schema_destructive(&Schema::new()).await.unwrap();
    assert_eq!(
        dropped.statements,
        [r#"DROP VIEW "user_ids""#, r#"DROP TABLE "users""#]
    );
    assert!(db.push_schema(&Schema::new()).await.unwrap().is_empty());
}

/// A materialised view is its own object: it is created, refreshed when its definition
/// changes, and dropped with `DROP MATERIALIZED VIEW`.
#[tokio::test]
async fn materialised_views_converge() {
    let db = common::database().await;
    common::reset(&db).await;

    let mut desired = schema(
        vec![table(
            "users",
            vec![{
                let mut id = column("id", TypeDef::Int8);
                id.primary_key = true;
                id
            }],
        )],
        Vec::new(),
    );
    let mut view = View::new("user_ids", "SELECT id FROM users");
    view.materialised = true;
    desired.add_view(view);

    let first = db.push_schema(&desired).await.unwrap();
    assert_eq!(
        first.statements,
        [
            r#"CREATE TABLE "users" ("id" BIGINT PRIMARY KEY)"#,
            r#"CREATE MATERIALIZED VIEW "user_ids" AS SELECT id FROM users"#,
        ]
    );
    assert!(db.introspect().await.unwrap().views[0].materialised);
    assert!(db.push_schema(&desired).await.unwrap().is_empty());

    let mut changed = desired.clone();
    changed.views[0].definition = "SELECT id FROM users WHERE id > 0".into();
    let replaced = db.push_schema(&changed).await.unwrap();
    // PostgreSQL cannot `CREATE OR REPLACE` a materialised view, so it is recreated and
    // refreshed.
    assert_eq!(
        replaced.statements,
        [
            r#"DROP MATERIALIZED VIEW "user_ids""#,
            r#"CREATE MATERIALIZED VIEW "user_ids" AS SELECT id FROM users WHERE id > 0"#,
            r#"REFRESH MATERIALIZED VIEW "user_ids""#,
        ]
    );
    assert!(db.push_schema(&changed).await.unwrap().is_empty());

    let dropped = db.push_schema_destructive(&Schema::new()).await.unwrap();
    assert_eq!(
        dropped.statements,
        [
            r#"DROP MATERIALIZED VIEW "user_ids""#,
            r#"DROP TABLE "users""#,
        ]
    );
}

/// An index's access method is part of its definition: a GIN index is created with
/// `USING gin`, read back as GIN, and replacing it with a b-tree is a drop and a create.
#[tokio::test]
async fn index_access_methods_round_trip() {
    let db = common::database().await;
    common::reset(&db).await;

    let base = |kind: IndexKind| {
        let mut events = table(
            "events",
            vec![
                {
                    let mut id = column("id", TypeDef::Int8);
                    id.primary_key = true;
                    id
                },
                column("payload", TypeDef::Jsonb),
            ],
        );
        events.indexes.push(Index {
            name: "events_payload_idx".into(),
            columns: vec!["payload".into()],
            unique: false,
            kind,
            predicate: None,
        });
        schema(vec![events], Vec::new())
    };

    let gin = base(IndexKind::Gin);
    let first = db.push_schema(&gin).await.unwrap();
    assert_eq!(
        first.statements,
        [
            r#"CREATE TABLE "events" ("id" BIGINT PRIMARY KEY, "payload" JSONB NOT NULL)"#,
            r#"CREATE INDEX "events_payload_idx" ON "events" USING gin ("payload")"#,
        ]
    );

    let introspected = db.introspect().await.unwrap();
    assert_eq!(
        introspected.table("events").unwrap().indexes[0].kind,
        IndexKind::Gin
    );
    assert!(db.push_schema(&gin).await.unwrap().is_empty());

    // A changed access method is a drop-then-create, because both statements use the name.
    // Dropping an index is a destructive change, so it needs the explicit form.
    let btree = base(IndexKind::Btree);
    let changed = db.push_schema_destructive(&btree).await.unwrap();
    assert_eq!(
        changed.statements,
        [
            r#"DROP INDEX "events_payload_idx""#,
            r#"CREATE INDEX "events_payload_idx" ON "events" ("payload")"#,
        ]
    );
    assert!(db.push_schema(&btree).await.unwrap().is_empty());
}

/// A standalone sequence is created, altered in place and dropped after the tables that use
/// it. The sequence behind an identity column is not a standalone one and is never reported.
#[tokio::test]
async fn sequences_are_created_altered_and_dropped() {
    let db = common::database().await;
    common::reset(&db).await;

    let mut desired = schema(
        vec![table(
            "orders",
            vec![{
                let mut id = column("id", TypeDef::Int8);
                id.primary_key = true;
                id.auto_increment = true;
                id
            }],
        )],
        Vec::new(),
    );
    let mut numbers = Sequence::new("order_number_seq");
    numbers.start = 1000;
    desired.add_sequence(numbers);

    let first = db.push_schema(&desired).await.unwrap();
    assert_eq!(
        first.statements,
        [
            r#"CREATE SEQUENCE "order_number_seq" AS bigint INCREMENT BY 1 START WITH 1000 CACHE 1 NO CYCLE"#,
            r#"CREATE TABLE "orders" ("id" BIGINT GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY)"#,
        ]
    );

    // The identity column's own sequence is not reported as a standalone one.
    let introspected = db.introspect().await.unwrap();
    assert_eq!(introspected.sequences.len(), 1);
    assert_eq!(introspected.sequences[0].name, "order_number_seq");
    assert_eq!(introspected.sequences[0].start, 1000);
    assert!(db.push_schema(&desired).await.unwrap().is_empty());

    // `ALTER SEQUENCE` describes the whole desired sequence.
    let mut changed = desired.clone();
    changed.sequences[0].start = 2000;
    changed.sequences[0].increment = 2;
    changed.sequences[0].cycle = true;
    let altered = db.push_schema(&changed).await.unwrap();
    assert_eq!(
        altered.statements,
        [
            r#"ALTER SEQUENCE "order_number_seq" AS bigint INCREMENT BY 2 NO MINVALUE NO MAXVALUE START WITH 2000 CACHE 1 CYCLE"#
        ]
    );
    assert_eq!(altered.changes[0].safety(), Safety::Risky);
    assert!(db.push_schema(&changed).await.unwrap().is_empty());

    // A table goes before the sequence it might default to.
    let dropped = db.push_schema_destructive(&Schema::new()).await.unwrap();
    assert_eq!(
        dropped.statements,
        [
            r#"DROP TABLE "orders""#,
            r#"DROP SEQUENCE "order_number_seq""#
        ]
    );
    assert!(db.push_schema(&Schema::new()).await.unwrap().is_empty());
}

mod generated {
    #![allow(unused_imports)]
    use besu::Table;

    /// A table whose `handle` column the database computes from `name`.
    #[derive(Table, Debug, Clone, PartialEq)]
    #[besu(table = "people")]
    #[allow(dead_code)]
    pub struct Person {
        #[besu(primary_key)]
        pub id: i64,
        pub name: String,
        #[besu(generated = "lower(name)")]
        pub handle: String,
    }
}

mod checked {
    #![allow(unused_imports)]
    use besu::Table;

    /// A table with both check spellings, including a boolean operator and a string literal
    /// that PostgreSQL reports with an explicit cast.
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

mod generated_more {
    #![allow(unused_imports)]
    use besu::Table;

    /// The same table with one more generated column, to exercise `ADD COLUMN`.
    #[derive(Table, Debug, Clone, PartialEq)]
    #[besu(table = "people")]
    #[allow(dead_code)]
    pub struct Person {
        #[besu(primary_key)]
        pub id: i64,
        pub name: String,
        #[besu(generated = "lower(name)")]
        pub handle: String,
        #[besu(generated = "upper(name)")]
        pub shout: String,
    }
}

/// A generated column is declared by the derive, rendered inside `CREATE TABLE`, read back
/// by introspection and never written by an insert.
#[tokio::test]
async fn generated_columns_round_trip() {
    let db = common::database().await;
    common::reset(&db).await;

    let first = db.push([generated::people.def()]).await.unwrap();
    assert_eq!(
        first.statements,
        [
            r#"CREATE TABLE "people" ("id" BIGINT PRIMARY KEY, "name" TEXT NOT NULL, "handle" TEXT GENERATED ALWAYS AS (lower(name)) STORED NOT NULL)"#
        ]
    );

    let introspected = db.introspect().await.unwrap();
    assert_eq!(
        introspected
            .table("people")
            .unwrap()
            .column("handle")
            .unwrap()
            .generated,
        Some(Generated {
            expression: "lower(name)".into(),
            stored: true,
        })
    );
    assert!(db.push([generated::people.def()]).await.unwrap().is_empty());

    // The generated column is not part of the insert struct, and the database fills it in.
    let insert = generated::people.insert(1, "Oscar");
    assert_eq!(
        <generated::PersonInsert as Insertable>::COLUMNS,
        ["id", "name"],
        "a generated column must never be written by an insert"
    );
    let _ = insert;

    // A whole-row update skips it for the same reason.
    let person = generated::Person {
        id: 1,
        name: "Oscar".into(),
        handle: "oscar".into(),
    };
    let update = db
        .update(generated::people)
        .set_model(&person)
        .filter(generated::people.id.eq(1))
        .to_sql()
        .unwrap();
    assert!(
        !update.sql().contains("handle"),
        "set_model must skip a generated column: {}",
        update.sql()
    );
    assert!(update.sql().contains("\"name\""), "{}", update.sql());

    db.execute(sql!("INSERT INTO people (id, name) VALUES (1, 'Oscar')"))
        .await
        .unwrap();
    let rows: Vec<String> = db
        .execute(sql!("SELECT handle FROM people"))
        .all()
        .await
        .unwrap();
    assert_eq!(rows, ["oscar".to_string()]);

    // Adding another generated column is one `ADD COLUMN`.
    let added = db.push([generated_more::people.def()]).await.unwrap();
    assert_eq!(
        added.statements,
        [
            r#"ALTER TABLE "people" ADD COLUMN "shout" TEXT GENERATED ALWAYS AS (upper(name)) STORED NOT NULL"#
        ]
    );
    assert_eq!(added.changes[0].safety(), Safety::Safe);
    assert!(
        db.push([generated_more::people.def()])
            .await
            .unwrap()
            .is_empty()
    );
}

// ---------------------------------------------------------------------------------------
// Introspection fidelity
// ---------------------------------------------------------------------------------------

/// A domain is a named type, and its base type is what a value is decoded through. Reading it
/// as `integer` would make the next push `ALTER ... TYPE` the column and destroy the domain,
/// so both halves are kept.
#[tokio::test]
async fn a_domain_round_trips_as_its_own_name_and_base_type() {
    let db = common::database().await;
    common::reset(&db).await;
    db.execute(besu::Fragment::new(
        "DROP DOMAIN IF EXISTS positive_integer CASCADE".to_owned(),
    ))
    .await
    .unwrap();
    db.execute(besu::Fragment::new(
        "CREATE DOMAIN positive_integer AS integer CHECK (VALUE > 0)".to_owned(),
    ))
    .await
    .unwrap();
    db.execute(besu::Fragment::new(
        "CREATE TABLE domain_holder (value positive_integer NOT NULL)".to_owned(),
    ))
    .await
    .unwrap();

    let introspected = db.introspect().await.unwrap();
    assert_eq!(
        introspected
            .table("domain_holder")
            .unwrap()
            .column("value")
            .unwrap()
            .ty,
        TypeDef::Domain(besu::DomainDef::new("positive_integer", &TypeDef::Int4))
    );

    // The snapshot replays to the schema that already exists: the base type is never named in
    // place of the domain.
    let pushed = db.push_schema(&introspected).await.unwrap();
    assert!(pushed.is_empty(), "got {:?}", pushed.changes);

    db.execute(besu::Fragment::new("DROP TABLE domain_holder".to_owned()))
        .await
        .unwrap();
    db.execute(besu::Fragment::new(
        "DROP DOMAIN positive_integer CASCADE".to_owned(),
    ))
    .await
    .unwrap();
}

/// `format_type` is what carries the modifier, so `timestamp(3)` and `bit(5)` survive a pull.
#[tokio::test]
async fn type_modifiers_are_kept() {
    let db = common::database().await;
    common::reset(&db).await;
    db.execute(besu::Fragment::new(
        "CREATE TABLE typed (\
             at TIMESTAMP(3) NOT NULL, \
             tod TIME(2) NOT NULL, \
             flags BIT(5) NOT NULL, \
             code VARCHAR(10) NOT NULL, \
             amount NUMERIC(10,2) NOT NULL)"
            .to_owned(),
    ))
    .await
    .unwrap();

    let introspected = db.introspect().await.unwrap();
    let table = introspected.table("typed").unwrap();
    assert_eq!(table.column("at").unwrap().ty, TypeDef::TimestampP(3));
    assert_eq!(table.column("tod").unwrap().ty, TypeDef::TimeP(2));
    assert_eq!(table.column("flags").unwrap().ty, TypeDef::Bit(5));
    assert_eq!(table.column("code").unwrap().ty, TypeDef::VarChar(10));
    assert_eq!(table.column("amount").unwrap().ty, TypeDef::Decimal(10, 2));

    // A push that dropped a modifier would change the column's precision silently.
    let pushed = db.push_schema(&introspected).await.unwrap();
    assert!(pushed.is_empty(), "got {:?}", pushed.changes);
}

/// A column collation is part of the column, so a pull has to keep it.
#[tokio::test]
async fn a_column_collation_round_trips() {
    let db = common::database().await;
    common::reset(&db).await;
    db.execute(besu::Fragment::new(
        "CREATE TABLE sorted (name text COLLATE \"C\" NOT NULL)".to_owned(),
    ))
    .await
    .unwrap();

    let introspected = db.introspect().await.unwrap();
    assert_eq!(
        introspected
            .table("sorted")
            .unwrap()
            .column("name")
            .unwrap()
            .ty,
        TypeDef::Collated(Box::new(TypeDef::Text), "C".into())
    );

    let pushed = db.push_schema(&introspected).await.unwrap();
    assert!(pushed.is_empty(), "got {:?}", pushed.changes);
}

/// A custom type name is quoted part by part, so its case reaches the server.
#[test]
fn a_custom_type_name_is_quoted() {
    use besu_core::Dialect;
    use besu_postgres::Postgres;

    assert_eq!(
        Postgres::type_name(TypeDef::Custom("MySchema.Mood".into())),
        r#""MySchema"."Mood""#
    );
    // A name the server accepts verbatim, such as an inline enum, is not mangled.
    assert_eq!(
        Postgres::type_name(TypeDef::Custom("enum('a','b')".into())),
        "enum('a','b')"
    );
}
