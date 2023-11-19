mod schema;

use crate::schema::{init, users};

// SELECT * FROM table_name WHERE condition;
#[tokio::test]
async fn select() {
    let db = init().await;

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
