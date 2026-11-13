//! A declared rename is one statement, and the column keeps its data.
#![cfg(feature = "sqlx")]
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]
#![allow(unused_imports)]

use besu::prelude::*;
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
