use besu::{table, Database};

pub use example_sqlite::{Post, User};

pub async fn init() -> Database {
    let db = Database::new(":memory:").await.unwrap();
    // TODO: Migrations

    db
}
