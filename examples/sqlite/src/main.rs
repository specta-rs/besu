//! A tour of Besu against SQLite.
//!
//! Run it with `cargo run -p example-sqlite`.

use besu::prelude::*;
use besu_sqlite::Sqlx;

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "users")]
pub struct User {
    /// The primary key. The database generates it, so `users.insert(..)` does not ask for it.
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    /// `UNIQUE` columns are declared here rather than with a separate index.
    #[besu(unique)]
    pub email: String,
    pub name: String,
    pub age: i32,
    /// Nullability comes from `Option`, and so does the Rust type of the column.
    pub bio: Option<String>,
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "posts")]
pub struct Post {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub title: String,
    /// A foreign key, which also feeds the migration system.
    #[besu(references = "users.id", on_delete = "cascade")]
    pub author_id: i64,
    #[besu(default = false)]
    pub published: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db = besu::Database::new(Sqlx::memory().await?);

    // Create both tables, or apply whatever has changed since the last run.
    //
    // This example owns its database, so it allows the destructive changes that happen when
    // a column or table is removed. A real application should use `push`, which refuses
    // those unless you call `push_destructive` explicitly.
    db.push_destructive([users.def(), posts.def()]).await?;

    // Start from an empty database so running the example twice behaves the same way twice.
    // `all_rows` is the explicit opt-in to a statement with no `WHERE`.
    db.delete(posts).all_rows().await?;
    db.delete(users).all_rows().await?;

    // Inserts return the rows the database actually wrote, including generated columns.
    let oscar = db
        .insert(users)
        .value(users.insert("oscar@example.com", "Oscar", 21))
        .returning(users)
        .await?
        .remove(0);

    let monty = db
        .insert(users)
        .value(
            users
                .insert("monty@example.com", "Monty", 7)
                .bio("Woof woof"),
        )
        .returning(users)
        .await?
        .remove(0);

    db.insert(posts)
        .values([
            posts.insert("Hello, world", oscar.id),
            posts.insert("A second post", oscar.id),
            posts.insert("Monty's first post", monty.id),
        ])
        .await?;

    // A typed select. The generated SQL is exactly what you would have written.
    println!("-- adults --");
    let adults: Vec<User> = db
        .select(users)
        .filter(users.age.gte(18))
        .order_by(users.name.asc())
        .await?;
    for user in &adults {
        println!("  {} ({})", user.name, user.email);
    }

    // Projections change the row type, so the compiler knows what comes back.
    println!("-- names and post counts --");
    let rows: Vec<(String, i64)> = db
        .select(users)
        .inner_join(posts, eq(users.id, posts.author_id))
        .pick((users.name, count(posts.id)))
        .group_by(users.name)
        .order_by(users.name.asc())
        .await?;
    for (name, count) in &rows {
        println!("  {name}: {count}");
    }

    // A left join hands back an `Option` for the joined side, because a row may not exist.
    println!("-- users and their posts --");
    let rows: Vec<(User, Option<Post>)> = db
        .select(users)
        .left_join(posts, eq(users.id, posts.author_id))
        .order_by(users.name.asc())
        .await?;
    for (user, post) in &rows {
        println!(
            "  {} -> {}",
            user.name,
            post.as_ref().map(|p| p.title.as_str()).unwrap_or("<none>")
        );
    }

    // Update in place, computed by the database rather than by Rust.
    db.update(users)
        .set_expr(users.age, users.age.as_expr().plus(1))
        .filter(users.id.eq(oscar.id))
        .await?;

    // Upsert: insert, or update when the unique column already exists.
    db.insert(users)
        .value(users.insert("oscar@example.com", "Oscar", 22))
        .on_conflict(users.email)
        .do_update(users.age, 22)
        .await?;

    // A transaction commits when the closure succeeds and rolls back when it returns an
    // error. `begin` is there for when you want to commit or roll back by hand.
    let mut tx = db.begin().await?;
    tx.insert(posts)
        .value(posts.insert("Will be rolled back", oscar.id))
        .await?;
    tx.rollback().await?;

    let posts_after_rollback: i64 = db.select(posts).pick(count_all()).await?.remove(0);
    println!("-- {posts_after_rollback} post(s) after the rollback --");

    // Raw SQL still binds its parameters, and `?` is rewritten for the target dialect.
    let filtered: Vec<User> = db
        .execute(sql!("SELECT * FROM users WHERE age > ? ORDER BY age", 10))
        .all()
        .await?;
    println!("-- {} user(s) older than 10 --", filtered.len());

    // Deleting without a filter does not compile; this is the explicit opt-in.
    db.delete(posts).filter(posts.published.eq(false)).await?;

    Ok(())
}
