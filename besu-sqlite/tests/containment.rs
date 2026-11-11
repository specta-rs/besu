//! Every operand of a comparison and every aggregate argument is checked against the query's
//! tables, not just the left-hand side.
//!
//! The failing cases are `compile_fail` doc tests in `besu-sqlite/src/lib.rs`; these are the
//! cases that must keep working.

use besu::prelude::*;
use besu_sqlite::Sqlx;

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "users")]
pub struct User {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub name: String,
    pub age: i32,
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "posts")]
pub struct Post {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub title: String,
    pub author_id: i64,
}

async fn db() -> besu::Database<Sqlx> {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.push([users.def(), posts.def()]).await.unwrap();
    db.insert(users)
        .values([users.insert("Alice", 30), users.insert("Bob", 20)])
        .await
        .unwrap();
    db.insert(posts)
        .values([posts.insert("Hello", 1), posts.insert("World", 1)])
        .await
        .unwrap();
    db
}

#[tokio::test]
async fn column_to_column_comparison_still_compiles_when_joined() {
    let db = db().await;
    let rows: Vec<(String, String)> = db
        .select(users)
        .inner_join(posts, eq(users.id, posts.author_id))
        .pick((users.name, posts.title))
        .order_by(posts.title.asc())
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, "Alice");
}

#[tokio::test]
async fn a_literal_on_the_left_is_checked_too() {
    let db = db().await;
    let rows: Vec<User> = db.select(users).filter(eq(7, users.age)).await.unwrap();
    assert!(rows.is_empty());
}

#[tokio::test]
async fn aggregate_arguments_are_checked() {
    let db = db().await;
    let rows: Vec<(String, i64)> = db
        .select(users)
        .inner_join(posts, eq(users.id, posts.author_id))
        .pick((users.name, count(posts.id)))
        .group_by(users.name)
        .await
        .unwrap();
    assert_eq!(rows, [("Alice".to_string(), 2)]);
}

#[tokio::test]
async fn helper_function_arguments_are_checked() {
    let db = db().await;
    let rows: Vec<User> = db
        .select(users)
        .filter(in_list(users.name, ["Alice"]))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);

    let rows: Vec<(i32, i64)> = db
        .select(users)
        .inner_join(posts, eq(users.id, posts.author_id))
        .pick((users.age, length(posts.title)))
        .await
        .unwrap();
    assert_eq!(rows[0].1, 5);
}
