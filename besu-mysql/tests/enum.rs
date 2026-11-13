//! An enum declared from Rust on MySQL: the labels are inline in the column, a value
//! round-trips, introspection reads the same labels back, and a changed label set is one
//! `ALTER COLUMN` rather than a rewritten table.
#![cfg(feature = "sqlx")]
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::prelude::*;
use besu::schema::{Change, Schema};
use besu::{EnumDef, TypeDef};
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

/// A Rust enum whose variants are the column's labels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, besu::SqlEnum)]
#[besu(rename_all = "snake_case")]
pub enum Mood {
    Happy,
    #[besu(rename = "meh")]
    Neutral,
    Sad,
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "reviews")]
pub struct Review {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    #[besu(sql_type = "Mood")]
    pub mood: Mood,
}

#[tokio::test]
async fn an_enum_round_trips_and_converges() {
    let db = fresh("besu_test_enum").await;

    let first = db.push([reviews.def()]).await.unwrap();
    assert!(
        first
            .statements
            .iter()
            .any(|statement| statement.contains("ENUM('happy','meh','sad')")),
        "the labels must be inline: {:?}",
        first.statements
    );
    // MySQL has no named type, so a declaration must not propose creating one.
    assert!(
        !first
            .changes
            .iter()
            .any(|change| matches!(change, Change::CreateEnum(_))),
        "MySQL has no named enum: {:?}",
        first.changes
    );

    db.insert(reviews)
        .value(reviews.insert(Mood::Neutral))
        .await
        .unwrap();

    let stored: Vec<Review> = db
        .select(reviews)
        .filter(reviews.mood.eq(Mood::Neutral))
        .await
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].mood, Mood::Neutral);
    assert!(
        db.select(reviews)
            .filter(reviews.mood.eq(Mood::Sad))
            .await
            .unwrap()
            .is_empty()
    );

    // The column introspects as an inline enum with the same labels, immediately after the
    // insert and again after the schema has been read back.
    let introspected = db.introspect().await.unwrap();
    assert!(introspected.enums.is_empty(), "MySQL has no named enums");
    assert_eq!(
        introspected
            .table("reviews")
            .unwrap()
            .column("mood")
            .unwrap()
            .ty,
        TypeDef::Enum(EnumDef::owned("", ["happy", "meh", "sad"]))
    );

    let second = db.push([reviews.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);
}

#[tokio::test]
async fn a_changed_label_set_is_an_alter_column() {
    let db = fresh("besu_test_enum_alter").await;
    db.push([reviews.def()]).await.unwrap();

    // Change one label without touching anything else. The diff has to be a single
    // `ALTER TABLE ... MODIFY COLUMN`, not a drop and re-add of the table.
    let mut desired = Schema::from_defs(&[reviews.def()]);
    let mood = desired.tables[0]
        .columns
        .iter_mut()
        .find(|column| column.name == "mood")
        .expect("the declared table has a mood column");
    mood.ty = TypeDef::Enum(EnumDef::owned("mood", ["happy", "meh", "ecstatic", "sad"]));

    let changes = db.push_schema(&desired).await.unwrap();
    assert_eq!(changes.changes.len(), 1, "got {:?}", changes.changes);
    assert!(
        matches!(&changes.changes[0], Change::AlterColumn { table, .. } if table == "reviews"),
        "got {:?}",
        changes.changes
    );
    assert!(
        changes
            .statements
            .iter()
            .all(|statement| statement.starts_with("ALTER TABLE `reviews` MODIFY COLUMN")),
        "got {:?}",
        changes.statements
    );

    let introspected = db.introspect().await.unwrap();
    assert_eq!(
        introspected
            .table("reviews")
            .unwrap()
            .column("mood")
            .unwrap()
            .ty,
        TypeDef::Enum(EnumDef::owned("", ["happy", "meh", "ecstatic", "sad"]))
    );
}
