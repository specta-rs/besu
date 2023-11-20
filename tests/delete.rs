use besu::operators::eq;
pub use schema::User;

use crate::schema::users;

mod schema;

// DELETE FROM table_name WHERE condition;

#[tokio::test]
async fn delete() {
    let db: besu::Database = schema::init().await;

    db.delete(users)._where(eq(users::id, 0u64)).await.unwrap();
    // TODO: Assert it was deleted correctly

    // db.delete(users)._where(gt(users::id, 0u64)).await.unwrap();
    // TODO: Assert it was deleted correctly

    // TODO: Trybuild test
    // db.delete(users)
    //     ._where(eq(schema::posts::id, 0u64))
    //     .await
    //     .unwrap();
}
