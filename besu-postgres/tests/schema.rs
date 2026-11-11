#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

//! Schema fidelity: native types, enum types, composite `UNIQUE` constraints and
//! schema-qualified tables, all exercised against a real PostgreSQL server.

mod common;

use besu::TypeDef;
use besu::prelude::*;
use besu::schema::{Change, Column, Enum, Safety, Schema, Table as TableIr, Unique};
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
        comment: None,
    }
}

fn schema(tables: Vec<TableIr>, enums: Vec<Enum>) -> Schema {
    let mut schema = Schema { tables, enums };
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
