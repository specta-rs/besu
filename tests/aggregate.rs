mod schema;

// SELECT COUNT(column_name) FROM table_name;
#[tokio::test]
async fn count() {
    let db = schema::init().await;

    // db.count(User::)

    // db.count(User::name).await.unwrap();
    // db.count(User::name)._where(gt(User::id, 5)).await.unwrap();

    // db.query()._where(gt(User::id, 5)).count().await.unwrap();

    // TODO
}

// SELECT SUM(column_name) FROM table_name;
#[tokio::test]
async fn sum() {
    let db = schema::init().await;

    // TODO
}
