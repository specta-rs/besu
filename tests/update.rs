use besu::Database;

pub use schema::User;

mod schema;

#[tokio::test]
async fn update() {
    let db = Database::new(":memory:");

    // db.query()._where(gt(User::id, 5));

    todo!();
}

#[tokio::test]
async fn update_many() {
    let db = Database::new(":memory:");

    todo!();
}
