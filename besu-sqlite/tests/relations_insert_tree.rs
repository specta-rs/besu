//! Inserting a parent and its has-many children together, against the default native driver.
//!
//! Every call is one transaction: the parent, then one multi-row child statement. The child
//! foreign key comes from the generated parent key, and a failed child statement leaves no
//! parent behind.
#![cfg(feature = "native")]
#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_arguments,
    clippy::type_complexity
)]

use besu::prelude::*;
use besu_sqlite::{Native, NativeDatabase, NativeError};

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

async fn setup() -> NativeDatabase {
    let db = Native::in_memory().unwrap().database();
    db.execute(sql!(
        r#"CREATE TABLE "users" (
            "id" INTEGER PRIMARY KEY AUTOINCREMENT,
            "email" TEXT NOT NULL UNIQUE,
            "name" TEXT NOT NULL
        )"#
    ))
    .await
    .unwrap();
    db.execute(sql!(
        r#"CREATE TABLE "posts" (
            "id" INTEGER PRIMARY KEY AUTOINCREMENT,
            "title" TEXT NOT NULL UNIQUE,
            "author_id" INTEGER NOT NULL REFERENCES "users"("id") ON DELETE CASCADE
        )"#
    ))
    .await
    .unwrap();
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

    // The placeholder `0` never reaches the database: both children carry the generated key.
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

    // And the rows are actually persisted, not just decoded from `RETURNING`.
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
                // `posts.title` is unique, so the second row fails and the whole statement
                // (and the parent insert before it) rolls back.
                posts.insert("duplicate", 0),
            ],
        )
        .await
        .unwrap_err();

    assert!(matches!(error, NativeError::Sqlite(_)));

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
        NativeError::Core(besu::Error::InvalidQuery(message))
            if message.contains("exactly one parent row")
    ));

    // Nothing was sent, so the parent table is still empty.
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
        NativeError::Core(besu::Error::InvalidQuery(message))
            if message.contains("exactly one parent row")
    ));
}
