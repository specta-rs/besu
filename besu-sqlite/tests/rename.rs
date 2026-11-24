//! A declared rename is one statement, and the column keeps its data.
#![cfg(any(feature = "native", feature = "sqlx"))]
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]
#![allow(unused_imports)]

use besu::prelude::*;
#[cfg(feature = "native")]
use besu_sqlite::Native;
#[cfg(feature = "sqlx")]
use besu_sqlite::Sqlx;

mod v1 {
    use besu::Table;

    #[derive(Table, Debug, Clone)]
    #[besu(table = "users")]
    pub struct User {
        #[besu(primary_key, auto_increment)]
        pub id: i64,
        pub full_name: String,
        pub age: i32,
    }
}

/// A table that never had the old names, so the rename declaration has nothing to match.
mod v0 {
    use besu::Table;

    #[derive(Table, Debug, Clone)]
    #[besu(table = "users")]
    pub struct User {
        #[besu(primary_key, auto_increment)]
        pub id: i64,
    }
}

mod v2 {
    use besu::Table;

    #[derive(Table, Debug, Clone)]
    #[besu(table = "users")]
    pub struct User {
        #[besu(primary_key, auto_increment)]
        pub id: i64,
        /// Renamed from `full_name`; without the attribute this would be a drop plus an add
        /// and the names would be lost.
        #[besu(column = "name", renamed_from = "full_name")]
        pub name: String,
        /// Renamed and widened in one push.
        #[besu(renamed_from = "age")]
        pub years: i64,
    }
}

#[tokio::test]
#[cfg(feature = "sqlx")]
async fn a_declared_rename_keeps_the_data() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());

    db.push([v1::users.def()]).await.unwrap();
    db.execute(sql!(
        "INSERT INTO users (id, full_name, age) VALUES (1, 'Oscar', 21)"
    ))
    .await
    .unwrap();

    let result = db.push([v2::users.def()]).await.unwrap();
    assert!(
        result
            .statements
            .iter()
            .any(|s| s.contains("RENAME COLUMN \"full_name\" TO \"name\"")),
        "expected a rename statement, got {:?}",
        result.statements
    );
    assert!(
        !result
            .changes
            .iter()
            .any(|c| matches!(c, besu::schema::Change::DropColumn { .. })),
        "a declared rename must not drop anything: {:?}",
        result.changes
    );

    let rows: Vec<(String, i64)> = db
        .execute(sql!("SELECT name, years FROM users"))
        .all()
        .await
        .unwrap();
    assert_eq!(rows, [("Oscar".to_string(), 21)]);

    // And the schema converges afterwards.
    assert!(db.push([v2::users.def()]).await.unwrap().is_empty());
}

#[tokio::test]
#[cfg(feature = "sqlx")]
async fn a_rename_that_did_not_happen_is_just_an_add() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.push([v0::users.def()]).await.unwrap();

    // Nothing was ever called `full_name` on this database, so the declaration has nothing to
    // match and the column is simply added.
    let result = db.push([v2::users.def()]).await.unwrap();
    assert!(
        result
            .statements
            .iter()
            .any(|s| s.contains("ADD COLUMN \"name\"")),
        "got {:?}",
        result.statements
    );
}

mod reuse_v0 {
    use besu::Table;

    #[derive(Table, Debug, Clone)]
    #[besu(table = "reused_names")]
    pub struct ReusedName {
        pub old: String,
    }
}

mod reuse_v1 {
    use besu::Table;

    #[derive(Table, Debug, Clone)]
    #[besu(table = "reused_names")]
    pub struct ReusedName {
        #[besu(column = "renamed", renamed_from = "old")]
        pub renamed: i64,
        #[besu(default = "fresh")]
        pub old: String,
    }
}

async fn a_rename_can_change_type_and_reuse_the_old_name<Dr>(driver: Dr)
where
    Dr: besu::Driver<Dialect = besu_sqlite::SQLite> + besu::SchemaDriver,
    Dr::Error: core::fmt::Debug,
    String: besu::Decode<Dr>,
    i64: besu::Decode<Dr>,
{
    let db = besu::Database::new(driver);
    db.push([reuse_v0::reused_names.def()]).await.unwrap();
    db.execute(sql!("INSERT INTO reused_names (old) VALUES ('41')"))
        .await
        .unwrap();

    let applied = db.push([reuse_v1::reused_names.def()]).await.unwrap();
    assert!(
        applied.changes.iter().any(|change| matches!(
            change,
            besu::schema::Change::RenameColumn { from, to, .. }
                if from == "old" && to == "renamed"
        )),
        "got {:?}",
        applied.changes
    );
    let rows: Vec<(String, i64)> = db
        .execute(sql!("SELECT old, renamed FROM reused_names"))
        .all()
        .await
        .unwrap();
    assert_eq!(rows, [("fresh".to_owned(), 41)]);
    assert!(
        db.push([reuse_v1::reused_names.def()])
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
#[cfg(feature = "native")]
async fn native_a_rename_can_change_type_and_reuse_the_old_name() {
    a_rename_can_change_type_and_reuse_the_old_name(Native::in_memory().unwrap()).await;
}

#[tokio::test]
#[cfg(feature = "sqlx")]
async fn sqlx_a_rename_can_change_type_and_reuse_the_old_name() {
    a_rename_can_change_type_and_reuse_the_old_name(Sqlx::memory().await.unwrap()).await;
}
