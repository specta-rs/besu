use besu::Database;

pub use schema::User;

// use crate::schema::Post;

mod schema;

// UPDATE table_name SET column1 = value1, column2 = value2, ... WHERE condition;

#[tokio::test]
async fn update() {
    let db = Database::new(":memory:").await.unwrap();

    // db.update(User);

    // db.update(User)
    //     .set(User::enabled, true)
    //     ._where(gt(User::id, 5))
    //     .await;

    // // TODO: Assert it won't compile
    // db.update(User).set(User::enabled, true).set(Post::id, true);

    // db.query(User)
    //     ._where(gt(User::id, 5))
    //     .update()
    //     .set(User::enabled, true)
    //     .await;

    todo!();
}

#[tokio::test]
async fn update_many() {
    let db = Database::new(":memory:").await.unwrap();

    todo!();
}

// TODO: https://orm.drizzle.team/docs/update#update-with-returning
