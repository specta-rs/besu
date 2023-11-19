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

pub async fn init() -> Database {
    let db = Database::new(":memory:").await.unwrap();
    // TODO: Migrations
    // TODO: Seed data

    db
}
