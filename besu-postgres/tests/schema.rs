#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

//! Schema fidelity: native types, enum types, composite `UNIQUE` constraints,
//! schema-qualified tables, check constraints, views and the index shapes the IR cannot
//! express, all exercised against a real PostgreSQL server.

mod common;

use besu::TypeDef;
use besu::prelude::*;
use besu::schema::{Change, Check, Column, Enum, Safety, Schema, Table as TableIr, Unique};
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
            r#"CREATE TABLE "reviews" ("id" BIGINT PRIMARY KEY, "mood" mood NOT NULL)"#,
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

/// A partial index is modelled, an expression index is not. Pushing a schema that matches
/// what is really there must not try to drop either: the expression index is invisible to
/// the IR, and the partial one is compared with its predicate.
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

    let introspected = db.introspect().await.unwrap();
    let events = introspected.table("events").unwrap();
    assert_eq!(
        events.indexes.len(),
        1,
        "the expression index must not be reported: {:?}",
        events.indexes
    );
    let partial = &events.indexes[0];
    assert_eq!(partial.name, "events_kind_idx");
    assert_eq!(partial.columns, ["kind"]);
    assert!(!partial.unique);
    let predicate = partial
        .predicate
        .as_deref()
        .expect("a partial index must carry its predicate");
    assert!(predicate.contains("error"), "got {predicate}");

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
            expression: "balance >= 0".into(),
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

/// A view is reported by introspection so `besu pull` writes it down, but it is never a
/// table and the differ never creates or drops one.
#[tokio::test]
async fn a_view_is_reported_but_never_touched() {
    let db = common::database().await;
    common::reset(&db).await;

    db.execute(sql!("CREATE TABLE users (id BIGINT PRIMARY KEY)"))
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
        introspected.views[0].definition.contains("users"),
        "got {:?}",
        introspected.views[0].definition
    );

    let desired = schema(
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
    let pushed = db.push_schema(&desired).await.unwrap();
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
