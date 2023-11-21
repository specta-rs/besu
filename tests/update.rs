use besu::operators::eq;

use crate::schema::{assert_sql, users};

mod schema;

// UPDATE table_name SET column1 = value1, column2 = value2, ... WHERE condition;

#[tokio::test]
async fn update() {
    let db = schema::init().await;

    {
        let query = db
            .update(users)
            .set(users::name, "My New Name")
            ._where(eq(users::id, 0u64));
        assert_sql(
            query.to_sql(),
            "UPDATE users SET name = ? WHERE users.id = ?",
            vec!["My New Name".into(), 0u64.into()],
        );
        query.await.unwrap();
        // TODO: Assert it updated correctly
    }

    {
        let query = db.update(users).set(users::name, "My New Name");
        assert_sql(
            query.to_sql(),
            "UPDATE users SET name = ?",
            vec!["My New Name".into()],
        );
        query.await.unwrap();
        // TODO: Assert it updated correctly
    }

    {
        let query = db
            .update(users)
            .set(users::name, "My New Name")
            ._where(eq(users::id, 0u64));
        assert_sql(
            query.to_sql(),
            "UPDATE users SET name = ? WHERE users.id = ?",
            vec!["My New Name".into(), 0u64.into()],
        );
        query.await.unwrap();
        // TODO: Assert it updated correctly
    }

    // TODO: Update many style thing

    // TODO: Trybuild test
    // db.update(users)
    //     .set(posts::title, "My New Name")
    //     .await
    //     .unwrap();
}

// TODO: https://orm.drizzle.team/docs/update#update-with-returning
