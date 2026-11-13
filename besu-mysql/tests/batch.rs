//! The batch API against a real MySQL server.
//!
//! MySQL has no `RETURNING`, so a batch that wants to read what it just wrote does it with a
//! plain `INSERT` followed by a `SELECT` in the same transaction — which is exactly the shape
//! this file pins down.
#![cfg(feature = "sqlx")]
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::prelude::*;
use besu_mysql::Sqlx;
use sqlx::MySqlPool;

/// The MySQL server the suite runs against.
const SERVER: &str = "mysql://root:besu@localhost:53306";

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "users")]
pub struct User {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    #[besu(unique)]
    pub email: String,
    pub name: String,
    pub age: i32,
    pub bio: Option<String>,
}

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

/// An empty database with the `users` table already pushed.
async fn setup(name: &str) -> besu::Database<Sqlx> {
    let db = fresh(name).await;
    db.push([users.def()]).await.unwrap();
    db
}

#[tokio::test]
async fn a_select_sees_the_write_before_it() {
    let db = setup("besu_test_batch_select_sees_write").await;

    // MySQL cannot return the inserted row, so the select is how the batch reads it back.
    // It runs in the same transaction, so the row is visible before the commit.
    let (first, second, rows): (_, _, Vec<User>) = db
        .batch((
            db.insert(users)
                .value(users.insert("a@example.com", "Alice", 30)),
            db.insert(users)
                .value(users.insert("b@example.com", "Bob", 17)),
            db.select(users)
                .filter(users.age.gte(18))
                .order_by(users.email.asc()),
        ))
        .await
        .unwrap();

    assert_eq!(first.rows_affected(), 1);
    assert_eq!(second.rows_affected(), 1);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].email, "a@example.com");
    assert_eq!(rows[0].age, 30);
    assert_eq!(rows[0].id, 1);

    let all: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn a_select_in_a_batch_decodes_several_columns_and_nulls() {
    let db = setup("besu_test_batch_select_columns").await;
    db.insert(users)
        .value(
            users
                .insert("b@example.com", "Bob", 17)
                .bio("hello".to_string()),
        )
        .await
        .unwrap();

    let (inserted, rows): (_, Vec<(String, Option<String>, i32)>) = db
        .batch((
            db.insert(users)
                .value(users.insert("a@example.com", "Alice", 30)),
            db.select(users)
                .pick((users.email, users.bio, users.age))
                .order_by(users.email.asc()),
        ))
        .await
        .unwrap();

    assert_eq!(inserted.rows_affected(), 1);
    assert_eq!(
        rows,
        vec![
            ("a@example.com".to_string(), None, 30),
            ("b@example.com".to_string(), Some("hello".to_string()), 17),
        ]
    );
}

#[tokio::test]
async fn a_failure_after_a_select_rolls_the_write_back() {
    let db = setup("besu_test_batch_select_rollback").await;
    db.insert(users)
        .value(users.insert("taken@example.com", "Taken", 20))
        .await
        .unwrap();

    let result = db
        .batch((
            db.insert(users)
                .value(users.insert("a@example.com", "Alice", 30)),
            // Reads the row the insert above wrote, inside the same transaction.
            db.select(users),
            // The unique index rejects this one, which must undo the write the select saw.
            db.insert(users)
                .value(users.insert("taken@example.com", "Duplicate", 31)),
        ))
        .await;

    assert!(result.is_err());

    let all: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].email, "taken@example.com");
}

#[tokio::test]
async fn two_selects_return_two_results() {
    let db = setup("besu_test_batch_two_selects").await;
    for (email, name, age) in [
        ("a@example.com", "Alice", 30),
        ("b@example.com", "Bob", 17),
        ("c@example.com", "Carol", 25),
    ] {
        db.insert(users)
            .value(users.insert(email, name, age))
            .await
            .unwrap();
    }

    let (everyone, adults): (Vec<User>, Vec<User>) = db
        .batch((
            db.select(users).order_by(users.age.asc()),
            db.select(users)
                .filter(users.age.gte(18))
                .order_by(users.age.asc()),
        ))
        .await
        .unwrap();

    assert_eq!(
        everyone
            .iter()
            .map(|user| user.name.as_str())
            .collect::<Vec<_>>(),
        ["Bob", "Carol", "Alice"]
    );
    assert_eq!(
        adults
            .iter()
            .map(|user| user.name.as_str())
            .collect::<Vec<_>>(),
        ["Carol", "Alice"]
    );
}
