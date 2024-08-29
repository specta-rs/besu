use besu::{Bruh, Database, Table};
use besu_sqlite::Sqlx;

#[derive(Debug, Table)]
// #[table(insert = NewUser)]
pub struct User {
    pub id: i32,
    pub name: String,
    pub age: i32,
    pub bio: String,
    pub phone: Option<String>,
}

#[derive(Debug, Table)]
pub struct Post {
    pub id: i32,
    pub title: String,
    pub author_id: i32,
}

#[tokio::main]
async fn main() {
    let db = Database::new(Sqlx::new(":memory:").await.unwrap());

    // TODO: Rename the table name to lower-case
    db.execute(
        "CREATE TABLE User (id INTEGER PRIMARY KEY, name TEXT, age INTEGER, bio TEXT, phone TEXT)",
        &[],
    )
    .await
    .unwrap();
    // TODO: insert many???
    for i in 0..10 {
        // TODO: Make this work without `SqliteValue` -> Fix `execute` method!!!
        db.execute(
            // TODO: We love SQL injection!!!
            &format!(
                "INSERT INTO User (id, name, age, bio, phone) VALUES ({}, '{}', {}, '{}', '{}')",
                i,
                "Alice",
                20 + i,
                "I am Alice",
                "1234567890"
            ),
            // "INSERT INTO User (id, name, age, bio, phone) VALUES (?, ?, ?, ?, ?)",
            &[
                // i.into(),
                // "Alice".into(),
                // (20 + i).into(),
                // "I am Alice".into(),
                // "1234567890".into(),
            ],
        )
        .await
        .unwrap();
    }

    println!("{:#?}", db.select(User).await.unwrap());

    // println!("{:#?}", db.select(User)._where(User.id, 5).await.unwrap());
}
