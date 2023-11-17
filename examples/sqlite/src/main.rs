use besu::{table, Database};

#[tokio::main]
async fn main() {
    let db = Database::new(":memory:").await.unwrap();
}
