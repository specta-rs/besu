mod schema;

use besu::{eq, WhereCondition};

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

// SELECT * FROM table_name WHERE condition;
#[tokio::test]
async fn select() {
    let db = init().await;

    let _r: Vec<User> = db.query(users)._where(eq(users::id, 5u64)).await.unwrap();

    // let _r: Vec<User> = db
    //     .query(users)
    //     ._where(gt(users::id, 5u64))
    //     ._where(lt(users::id, 10u64))
    //     .await
    //     .unwrap();

    // TODO: What if `WhereCondition` is for some random table. We need to enforce it's the users table!
    // TODO: `Vec<(User, Option<Post>)>`
    let _r = db
        .query(users)
        .left_join(posts, eq(users::id, posts::owner_id));
    // .await
    // .unwrap();

    let _r = db
        .query(users)
        .left_join(posts, eq(users::id, comments::author_id));

    // TODO: Enforce column type is the same when comparing them
    // TODO: What if left join by hardcoded value and not a column?
    // TODO: What if left join by a column from unrelated table?
}

#[tokio::test]
async fn select_inner_join() {
    // TODO
}

// TODO: Select while joining with other tables
