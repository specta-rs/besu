//! Per-parent relation limits and ordering against the default native SQLite driver.
//!
//! The sqlx-backed suite in `relations.rs` covers the same behaviour with the compatibility
//! driver; this file keeps the native default build honest without `--all-features`.
#![cfg(feature = "native")]
#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_arguments,
    clippy::type_complexity
)]

use besu::prelude::*;
use besu::types::Int8;
use besu_sqlite::{Native, NativeDatabase};

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "users")]
pub struct User {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub name: String,
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "posts")]
pub struct Post {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub title: String,
    pub author_id: i64,
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "comments")]
pub struct Comment {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub body: String,
    pub post_id: i64,
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "tags")]
pub struct Tag {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub name: String,
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "post_tags")]
pub struct PostTag {
    #[besu(primary_key)]
    pub post_id: i64,
    #[besu(primary_key)]
    pub tag_id: i64,
}

const TAGS: ManyToMany<PostTable, PostTagTable, TagTable, Int8, Int8> =
    many_to_many(posts.id, post_tags.post_id, post_tags.tag_id, tags.id);

async fn setup() -> NativeDatabase {
    let db = Native::in_memory().unwrap().database();
    db.execute(sql!(
        r#"CREATE TABLE "users" (
            "id" INTEGER PRIMARY KEY AUTOINCREMENT,
            "name" TEXT NOT NULL
        )"#
    ))
    .await
    .unwrap();
    db.execute(sql!(
        r#"CREATE TABLE "posts" (
            "id" INTEGER PRIMARY KEY AUTOINCREMENT,
            "title" TEXT NOT NULL,
            "author_id" INTEGER NOT NULL REFERENCES "users"("id") ON DELETE CASCADE
        )"#
    ))
    .await
    .unwrap();
    db.execute(sql!(
        r#"CREATE TABLE "comments" (
            "id" INTEGER PRIMARY KEY AUTOINCREMENT,
            "body" TEXT NOT NULL,
            "post_id" INTEGER NOT NULL REFERENCES "posts"("id") ON DELETE CASCADE
        )"#
    ))
    .await
    .unwrap();
    db.execute(sql!(
        r#"CREATE TABLE "tags" (
            "id" INTEGER PRIMARY KEY AUTOINCREMENT,
            "name" TEXT NOT NULL
        )"#
    ))
    .await
    .unwrap();
    db.execute(sql!(
        r#"CREATE TABLE "post_tags" (
            "post_id" INTEGER NOT NULL REFERENCES "posts"("id") ON DELETE CASCADE,
            "tag_id" INTEGER NOT NULL REFERENCES "tags"("id") ON DELETE CASCADE
        )"#
    ))
    .await
    .unwrap();
    db
}

/// Three users, two posts for Alice, one for Bob and none for Carol.
async fn seed(db: &NativeDatabase) {
    for name in ["Alice", "Bob", "Carol"] {
        db.insert(users).value(users.insert(name)).await.unwrap();
    }
    for (title, author) in [("Alice one", 1), ("Alice two", 1), ("Bob one", 2)] {
        db.insert(posts)
            .value(posts.insert(title, author))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_has_many_limit_is_per_parent() {
    let db = setup().await;
    seed(&db).await;

    let loaded: Vec<(User, Vec<Post>)> = db
        .select(users)
        .order_by(users.id.asc())
        .with(
            has_many(users.id, posts.author_id)
                .order_by(posts.id.desc())
                .limit(1),
        )
        .await
        .unwrap();

    assert_eq!(
        loaded[0]
            .1
            .iter()
            .map(|post| post.title.as_str())
            .collect::<Vec<_>>(),
        ["Alice two"]
    );
    assert_eq!(loaded[1].1[0].title, "Bob one");
    assert!(loaded[2].1.is_empty());
}

#[tokio::test]
async fn a_many_to_many_limit_is_per_parent() {
    let db = setup().await;
    seed(&db).await;

    for name in ["rust", "sql", "orm"] {
        db.insert(tags).value(tags.insert(name)).await.unwrap();
    }
    for (post, tag) in [(1, 1), (1, 2), (2, 1), (3, 3)] {
        db.insert(post_tags)
            .value(post_tags.insert(post, tag))
            .await
            .unwrap();
    }

    let loaded: Vec<(Post, Vec<Tag>)> = db
        .select(posts)
        .order_by(posts.id.asc())
        .with(TAGS.order_by(tags.name.desc()).limit(1))
        .await
        .unwrap();

    assert_eq!(
        loaded[0]
            .1
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>(),
        ["sql"]
    );
    assert_eq!(loaded[1].1[0].name, "rust");
    assert_eq!(loaded[2].1[0].name, "orm");
}

#[tokio::test]
async fn nested_levels_each_have_their_own_limit() {
    let db = setup().await;
    seed(&db).await;

    for (body, post) in [
        ("Alice one first", 1),
        ("Alice one second", 1),
        ("Bob one first", 3),
    ] {
        db.insert(comments)
            .value(comments.insert(body, post))
            .await
            .unwrap();
    }

    let loaded: Vec<(User, Vec<(Post, Vec<Comment>)>)> = db
        .select(users)
        .order_by(users.id.asc())
        .with(
            has_many(users.id, posts.author_id)
                .order_by(posts.id.asc())
                .limit(2)
                .with(
                    has_many(posts.id, comments.post_id)
                        .order_by(comments.id.desc())
                        .limit(1),
                ),
        )
        .await
        .unwrap();

    assert_eq!(loaded[0].1.len(), 2);
    assert_eq!(loaded[0].1[0].1[0].body, "Alice one second");
    assert!(loaded[0].1[1].1.is_empty());
    assert_eq!(loaded[1].1[0].1[0].body, "Bob one first");
    assert!(loaded[2].1.is_empty());
}
