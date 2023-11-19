use std::path::PathBuf;

use besu::{table, Database};

// pub use example_sqlite::{posts, users, Post, User};

table!(User, users, {
    id: u64,
    name: String,
    age: u8,
    bio: String; @default("Cool"),
    phone: Option<String>,
    // posts: Many<Post>,
},
// Right now the macro isn't very smart. The following will all be derived from the above.
{
    // Strip PK, change to `Option` if has `@default`
    insert: {
        name: String,
        age: u8,
        bio: Option<String>,
        phone: Option<String>,
    },
});

table!(Post, posts, {
    id: u64,
    title: String,
    owner_id: u64,
},
// Right now the macro isn't very smart. The following will all be derived from the above.
{
    // Strip PK, change to `Option` if has `@default`
    insert: {
        title: String,
        owner_id: u64, // TODO: Enforce foreign key within type system
    },
});

table!(Comment, comments, {
    id: u64,
    title: String,
    author_id: u64,
},
// Right now the macro isn't very smart. The following will all be derived from the above.
{
    // Strip PK, change to `Option` if has `@default`
    insert: {
        title: String,
        author_id: u64, // TODO: Enforce foreign key within type system
    },
});

pub async fn init() -> Database {
    let path = PathBuf::from(":memory:");
    // let path = PathBuf::from("./debug.db");

    if path.exists() {
        std::fs::remove_file(&path).unwrap();
    }

    let db = Database::new(path).await.unwrap();
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

    // TODO: Seed data

    db
}
