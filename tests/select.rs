mod schema;

use besu::{
    operators::{eq, WhereCondition},
    queries::Query,
    SqlValue,
};

use crate::schema::{comments, init, posts, users, Post, User};

#[tokio::test]
async fn conditions() {
    let _: WhereCondition<User> = eq(users::id, 5u64);
    let _: WhereCondition<User> = eq(5u64, users::id);
    // let _: WhereCondition<User> = eq(5.0f64, users::id); // Won't compile. type mismatch
    // let _: WhereCondition<User> = eq(users::id, 5.0f64); // Won't compile. type mismatch
    // let _: WhereCondition<User> = eq(5.0f64, 64u32); // Won't compile. type mismatch
    // let _: WhereCondition<(User, Post)> = eq(users::id, posts::title); // Won't compile. type mismatch
    let _: WhereCondition<(User, Post)> = eq(users::id, posts::owner_id);
    // let _: WhereCondition<User> = eq(users::id, posts::owner_id); // Won't compile. Missing `Post`.
}

fn assert_sql(
    v: Result<(String, Vec<SqlValue>), ()>,
    expected_query: impl Into<String>,
    expected_values: Vec<SqlValue>,
) {
    let (query, values) = v.unwrap();
    assert_eq!(query, expected_query.into());
    assert_eq!(values, expected_values);
}

// SELECT * FROM table_name WHERE condition;
#[tokio::test]
async fn select() {
    let db = init().await;

    let query = db.query(users)._where(eq(users::id, 5u64));
    assert_sql(
        query.to_sql(),
        "SELECT id, name, age, bio, phone FROM User WHERE User.id = 5",
        vec![],
    );
    let users2: Vec<User> = query.await.unwrap(); // TODO: You can't have a variable called `users` or a function argument because it conflicts with the constant. This is a big gotcha.
    assert_eq!(users2.len(), 0); // TODO: Actually assert some values. We need to seed the DB first.

    // let (query, values) = db
    //     .query(users)
    //     ._where(eq(users::id, 5u64))
    //     .to_sql()
    //     .unwrap();
    // assert_eq!(
    //     query,
    //     "SELECT id, name, age, bio, phone FROM User WHERE User.id = 5"
    // );
    // assert_eq!(values, vec![]);

    // let _r: Vec<User> = db.query(users)._where(eq(users::id, 5u64)).await.unwrap();

    // let _r: Vec<User> = db
    //     .query(users)
    //     ._where(gt(users::id, 5u64))
    //     ._where(lt(users::id, 10u64))
    //     .await
    //     .unwrap();

    // let _r: Vec<(User, Option<Post>)> = db
    //     .query(users)
    //     // ._where(eq(posts::title, "Hello World".into())) // Invalid because we haven't joined posts, yet
    //     .left_join(posts, eq(users::id, posts::owner_id))
    //     ._where(eq(posts::title, "Hello World".to_string()))
    //     .await
    //     .unwrap();

    // let _r: Query<(User, Option<Post>)> = db
    //     .query(users)
    //     .left_join(posts, eq(users::id, comments::author_id)); // Invalid because `comments` is never refered to (either in the `query` or `inner_join`)

    // You can't join a table multiple times. This is a runtime error
    // let _r = db
    //     .query(users)
    //     .left_join(posts, eq(users::id, posts::owner_id))
    //     .left_join(posts, eq(users::id, 5u64))
    //     .await
    //     .unwrap();
    // TODO: Redo this test with mismatch of join types
}

#[tokio::test]
async fn select_inner_join() {
    // TODO
}

// TODO: Select while joining with other tables
