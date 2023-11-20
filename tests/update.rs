use besu::operators::eq;

use crate::schema::users;

mod schema;

// UPDATE table_name SET column1 = value1, column2 = value2, ... WHERE condition;

#[tokio::test]
async fn update() {
    let db = schema::init().await;

    db.update(users)
        .set(users::name, "My New Name")
        ._where(eq(users::id, 0u64))
        .await
        .unwrap();
    // TODO: Assert it updated correctly

    // Update all values
    db.update(users)
        .set(users::name, "My New Name")
        .await
        .unwrap();
    // TODO: Assert it updated correctly

    // TODO: Update many style thing

    // TODO: Trybuild test
    // db.update(users)
    //     .set(posts::title, "My New Name")
    //     .await
    //     .unwrap();
}

// TODO: https://orm.drizzle.team/docs/update#update-with-returning
