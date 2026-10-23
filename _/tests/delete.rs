use besu::operators::eq;
pub use schema::User;

use crate::schema::{assert_sql, users};

mod schema;

// DELETE FROM table_name WHERE condition;

#[tokio::test]
async fn delete() {
    let db: besu::Database = schema::init().await;

    {
        let query = db.delete(users)._where(eq(users::id, 0u64));
        assert_sql(
            query.to_sql(),
            "DELETE FROM users WHERE users.id = ?",
            vec![0u64.into()],
        );
        query.await.unwrap();
        // TODO: Assert it was deleted correctly
    }

    {
        let query = db
            .delete(users)
            ._where(eq(users::id, 0u64))
            ._where(eq(users::name, "Hacker".to_string()));
        assert_sql(
            query.to_sql(),
            "DELETE FROM users WHERE users.id = ? AND users.name = ?",
            vec![0u64.into(), "Hacker".into()],
        );
        query.await.unwrap();
        // TODO: Assert it was deleted correctly
    }

    // db.delete(users)._where(gt(users::id, 0u64)).await.unwrap();
    // TODO: Assert it was deleted correctly

    // TODO: Trybuild test
    // db.delete(users)
    //     ._where(eq(schema::posts::id, 0u64))
    //     .await
    //     .unwrap();
}
