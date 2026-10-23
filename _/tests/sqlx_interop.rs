use besu::Database;

use sqlx::query;

#[tokio::test]
async fn sqlx_interop() {
    let db = Database::new(":memory:").await.unwrap();

    assert_eq!(
        query!("SELECT 1 as num")
            .fetch_one(db.sqlite())
            .await
            .unwrap()
            .num,
        1
    );
}
