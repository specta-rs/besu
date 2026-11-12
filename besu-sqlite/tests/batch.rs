//! The batch API against a real SQLite database.
//!
//! The point of these tests is the guarantee, not the ergonomics: a batch either lands
//! whole or leaves nothing behind. The generated-SQL test pins the text each statement in
//! the tuple renders to, which is also the text the driver sends.
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::Batch;
use besu::prelude::*;
use besu_sqlite::Sqlx;

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
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.execute(sql!(
        r#"CREATE TABLE "users" (
            "id" INTEGER PRIMARY KEY AUTOINCREMENT,
            "email" TEXT NOT NULL UNIQUE,
            "name" TEXT NOT NULL,
            "age" INTEGER NOT NULL,
            "bio" TEXT
        )"#
    ))
    .await
    .unwrap();
    db
}

#[tokio::test]
async fn three_inserts_land_together() {
    let db = setup().await;

    // The tuple is the return value: each element is that statement's own output, in the
    // order it was listed. `.rows_affected()` pins the output type to the driver's.
    let (first, second, third) = db
        .batch((
            db.insert(users)
                .value(users.insert("a@example.com", "Alice", 30)),
            db.insert(users)
                .value(users.insert("b@example.com", "Bob", 31)),
            db.update(users)
                .set(users.age, 40)
                .filter(users.email.eq("a@example.com")),
        ))
        .await
        .unwrap();

    assert_eq!(first.rows_affected(), 1);
    assert_eq!(second.rows_affected(), 1);
    assert_eq!(third.rows_affected(), 1);

    let all: Vec<User> = db.select(users).order_by(users.email.asc()).await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].email, "a@example.com");
    assert_eq!(all[0].age, 40);
    assert_eq!(all[1].email, "b@example.com");
    assert_eq!(all[1].age, 31);
}

#[tokio::test]
async fn a_failure_in_the_middle_inserts_nothing() {
    let db = setup().await;

    // A row that the batch's second statement will collide with.
    db.insert(users)
        .value(users.insert("taken@example.com", "Taken", 20))
        .await
        .unwrap();

    let result = db
        .batch((
            db.insert(users)
                .value(users.insert("a@example.com", "Alice", 30)),
            // The unique index rejects this one, which must undo the insert above it.
            db.insert(users)
                .value(users.insert("taken@example.com", "Duplicate", 31)),
            db.insert(users)
                .value(users.insert("c@example.com", "Carol", 32)),
        ))
        .await;

    assert!(result.is_err());

    // Only the row that was already there remains: the whole batch rolled back.
    let all: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].email, "taken@example.com");
}

#[tokio::test]
async fn eight_statements_work() {
    let db = setup().await;

    let out = db
        .batch((
            db.insert(users)
                .value(users.insert("u0@example.com", "User 0", 0)),
            db.insert(users)
                .value(users.insert("u1@example.com", "User 1", 1)),
            db.insert(users)
                .value(users.insert("u2@example.com", "User 2", 2)),
            db.insert(users)
                .value(users.insert("u3@example.com", "User 3", 3)),
            db.insert(users)
                .value(users.insert("u4@example.com", "User 4", 4)),
            db.insert(users)
                .value(users.insert("u5@example.com", "User 5", 5)),
            db.insert(users)
                .value(users.insert("u6@example.com", "User 6", 6)),
            db.insert(users)
                .value(users.insert("u7@example.com", "User 7", 7)),
        ))
        .await
        .unwrap();

    assert_eq!(
        (
            out.0.rows_affected(),
            out.1.rows_affected(),
            out.2.rows_affected(),
            out.3.rows_affected(),
            out.4.rows_affected(),
            out.5.rows_affected(),
            out.6.rows_affected(),
            out.7.rows_affected(),
        ),
        (1, 1, 1, 1, 1, 1, 1, 1)
    );

    let all: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(all.len(), 8);
}

#[tokio::test]
async fn generated_sql_is_each_statement_to_sql() {
    let db = setup().await;

    // Rendering is public and does not run anything, so the exact text a batch would send
    // can be checked without watching the server. It is the same text each builder's
    // `to_sql` produces, because that is what `render` calls.
    let rendered = (
        db.insert(users)
            .value(users.insert("a@example.com", "Alice", 30)),
        db.insert(users)
            .value(users.insert("b@example.com", "Bob", 31)),
        db.update(users)
            .set(users.age, 40)
            .filter(users.email.eq("a@example.com")),
    )
        .render()
        .unwrap();

    assert_eq!(
        rendered.0.sql(),
        r#"INSERT INTO "users" ("email", "name", "age") VALUES (?, ?, ?)"#
    );
    assert_eq!(rendered.0.args().len(), 3);
    assert_eq!(
        rendered.1.sql(),
        r#"INSERT INTO "users" ("email", "name", "age") VALUES (?, ?, ?)"#
    );
    assert_eq!(
        rendered.2.sql(),
        r#"UPDATE "users" SET "age" = ? WHERE ("users"."email" = ?)"#
    );
    assert_eq!(rendered.2.args().len(), 2);
}
