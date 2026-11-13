//! Inserting a parent and its has-many children together, against a real PostgreSQL server.
//!
//! The SQLite suite in `besu-sqlite/tests/relations_insert_tree.rs` covers the same contract
//! with an in-memory database. These tests pin the PostgreSQL shape: `RETURNING` on both
//! statements, the generated identity used as the children's foreign key, and rollback via a
//! real unique constraint.
#![cfg(feature = "sqlx")]
#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_arguments,
    clippy::type_complexity
)]

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
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "posts")]
pub struct Post {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    #[besu(unique)]
    pub title: String,
    #[besu(references = "users.id", on_delete = "cascade")]
    pub author_id: i64,
}

async fn setup() -> besu::Database<Sqlx> {
    let db = common::database().await;
    common::reset(&db).await;
    db.push([users.def(), posts.def()]).await.unwrap();
    db
}

#[tokio::test]
async fn inserts_a_parent_and_its_children() {
    let db = setup().await;

    let (user, created) = db
        .insert_tree(
            db.insert(users)
                .value(users.insert("alice@example.com", "Alice")),
            has_many(users.id, posts.author_id),
            [posts.insert("Hello", 0), posts.insert("World", 0)],
        )
        .await
        .unwrap();

    assert!(user.id > 0);
    assert_eq!(user.email, "alice@example.com");
    assert_eq!(user.name, "Alice");

    assert_eq!(created.len(), 2);
    assert_eq!(
        created
            .iter()
            .map(|post| (post.title.as_str(), post.author_id))
            .collect::<Vec<_>>(),
        [("Hello", user.id), ("World", user.id)]
    );
    assert!(
        created
            .iter()
            .all(|post| post.id > 0 && post.author_id != 0)
    );

    let stored: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(stored, [user]);
    let stored_posts: Vec<Post> = db.select(posts).order_by(posts.id.asc()).await.unwrap();
    assert_eq!(stored_posts, created);
}

#[tokio::test]
async fn a_parent_without_children_returns_an_empty_vec() {
    let db = setup().await;

    let empty = [];
    let (user, created) = db
        .insert_tree(
            db.insert(users)
                .value(users.insert("carol@example.com", "Carol")),
            has_many(users.id, posts.author_id),
            empty,
        )
        .await
        .unwrap();

    assert!(user.id > 0);
    assert!(created.is_empty());
    assert_eq!(db.select(users).await.unwrap(), [user]);
    assert!(db.select(posts).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_failing_child_rolls_back_the_parent() {
    let db = setup().await;

    let error = db
        .insert_tree(
            db.insert(users)
                .value(users.insert("bob@example.com", "Bob")),
            has_many(users.id, posts.author_id),
            [
                posts.insert("duplicate", 0),
                // `posts.title` is unique, so the second row fails the whole
                // statement and the parent insert before it is rolled back with it.
                posts.insert("duplicate", 0),
            ],
        )
        .await
        .unwrap_err();

    assert!(matches!(error, sqlx::Error::Database(_)));

    assert!(db.select(users).await.unwrap().is_empty());
    assert!(db.select(posts).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_multi_row_parent_is_rejected_before_a_transaction_opens() {
    let db = setup().await;

    let error = db
        .insert_tree(
            db.insert(users).values([
                users.insert("a@example.com", "Alice"),
                users.insert("b@example.com", "Bob"),
            ]),
            has_many(users.id, posts.author_id),
            [posts.insert("Hello", 0)],
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        sqlx::Error::Protocol(message) if message.contains("exactly one parent row")
    ));

    assert!(db.select(users).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_row_count_mismatch_is_rejected() {
    let db = setup().await;

    let error = db
        .insert_tree(
            db.insert(users),
            has_many(users.id, posts.author_id),
            [posts.insert("Hello", 0)],
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        sqlx::Error::Protocol(message) if message.contains("exactly one parent row")
    ));
}
