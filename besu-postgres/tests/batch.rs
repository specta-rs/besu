//! The batch API against a real PostgreSQL server.
//!
//! The batch used to hold writes only. These tests pin the read half down where it matters:
//! a `SELECT` element runs inside the same transaction as the writes before it, decodes into
//! the tuple, and commits or rolls back with them.
#![cfg(feature = "sqlx")]
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

mod common;

use besu::prelude::*;
use besu_postgres::Sqlx;

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

async fn setup() -> besu::Database<Sqlx> {
    let db = common::database().await;
    common::reset(&db).await;
    db.push([users.def()]).await.unwrap();
    db
}

#[tokio::test]
async fn a_select_sees_the_write_before_it() {
    let db = setup().await;

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
    // The generated key came back in the row the select decoded, so the statement really ran
    // in the same transaction rather than against a snapshot from before the insert.
    assert_eq!(rows[0].id, 1);

    let all: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn a_returning_write_and_a_select_share_the_tuple() {
    let db = setup().await;

    // Both elements report rows, and they still keep their own result slots.
    let (created, everyone): (Vec<User>, Vec<User>) = db
        .batch((
            db.insert(users)
                .value(users.insert("a@example.com", "Alice", 30))
                .returning(users),
            db.select(users),
        ))
        .await
        .unwrap();

    assert_eq!(created.len(), 1);
    assert_eq!(created[0].email, "a@example.com");
    assert_eq!(everyone, created);
}

#[tokio::test]
async fn a_select_in_a_batch_decodes_several_columns_and_nulls() {
    let db = setup().await;
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
    let db = setup().await;
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
    let db = setup().await;
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

#[tokio::test]
async fn a_set_query_is_a_batch_element() {
    let db = setup().await;
    for (email, name, age) in [("a@example.com", "Alice", 30), ("b@example.com", "Bob", 17)] {
        db.insert(users)
            .value(users.insert(email, name, age))
            .await
            .unwrap();
    }

    let (union, counted): (Vec<String>, Vec<i64>) = db
        .batch((
            db.select(users)
                .pick(users.email)
                .filter(users.age.gte(18))
                .union(db.select(users).pick(users.email).filter(users.age.lt(18))),
            db.select(users).count(),
        ))
        .await
        .unwrap();

    assert_eq!(union.len(), 2);
    assert_eq!(counted, vec![2]);
}
