use std::path::PathBuf;

use besu::{table, Database, Error, SqlValue};

// pub use example_sqlite::{posts, users, Post, User};

table!(User, users, {
    id: |t| t.serial("id"),
    name: |t| t.text("name"),
    age: |t| t.int("age"), // TODO: Use `u8` not default `int` type???
    bio: |t| t.text("bio").default("Cool"),
    phone: |t| t.text("phone").optional(),
    // posts: Many<Post>,
});

table!(Post, posts, {
    id: |t| t.serial("id"),
    title: |t| t.text("title"),
    owner_id: |t| t.int("owner_id"), // TODO: Enforce foreign keys within type system
});

table!(Comment, comments, {
    id: |t| t.serial("id"),
    title: |t| t.text("title"),
    author_id: |t| t.int("author_id"),
});

pub async fn init() -> Database {
    let path = PathBuf::from(":memory:");
    // let path = PathBuf::from("./debug.db");

    if path.exists() {
        std::fs::remove_file(&path).unwrap();
    }

    let db = Database::new_with_logger(path, |sql| println!("EXECUTE: {sql}"))
        .await
        .unwrap();
    db.migrate(users).await.unwrap();
    db.migrate(posts).await.unwrap();

    db.execute(
        r#"INSERT INTO users(id, name, age, bio, phone) VALUES (0, "Monty Beaumont", 7, "Woof Woof", NULL) "#,
        vec![],
    )
    .await
    .unwrap();

    db.execute(
        r#"INSERT INTO users(id, name, age, bio, phone) VALUES (1, "Millie Beaumont", 1, "Woof Woof Woof", NULL) "#,
        vec![],
    )
    .await
    .unwrap();

    db.execute(
        r#"INSERT INTO users(id, name, age, bio, phone) VALUES (2, "Oscar Beaumont", 21, "Me like dog", NULL) "#,
        vec![],
    )
    .await
    .unwrap();

    db.execute(
        r#"INSERT INTO posts(id, title, owner_id) VALUES (0, 'Post One', 0) "#,
        vec![],
    )
    .await
    .unwrap();

    db.execute(
        r#"INSERT INTO posts(id, title, owner_id) VALUES (1, 'Post Two', 1) "#,
        vec![],
    )
    .await
    .unwrap();

    db.execute(
        r#"INSERT INTO posts(id, title, owner_id) VALUES (2, 'Post Three', 1) "#,
        vec![],
    )
    .await
    .unwrap();

    // TODO: Seed data

    db
}

#[track_caller]
pub fn assert_sql(
    v: Result<(String, Vec<SqlValue>), Error>,
    expected_query: impl Into<String>,
    expected_values: Vec<SqlValue>,
) {
    let (query, values) = v.unwrap();
    assert_eq!(query, expected_query.into());
    assert_eq!(values, expected_values);
}
