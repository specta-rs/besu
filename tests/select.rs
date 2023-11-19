mod schema;

use besu::eq;

use crate::schema::{init, users};

// SELECT * FROM table_name WHERE condition;
#[tokio::test]
async fn select() {
    let db = init().await;

    let y = users._where(eq(users::id, 5));

    // let y = users._where(eq(users::id, 5))._where(eq(users::id, 5));

    // TODO: Different type of joins

    // db.query(users._where(users::id, 5));

    // let user = db.query(users._where(()));

    // let user = db.query(users)._where(());

    // db.query(User)._where(()).count();
    // let _r: Vec<User> = db.query(User::model)._where(()).select().await.unwrap();

    // db.query(User)._where(()).limit(1).await.unwrap();

    // db.select(User).await.unwrap();
}

#[tokio::test]
async fn select_inner_join() {
    // TODO
}

// TODO: Select while joining with other tables
