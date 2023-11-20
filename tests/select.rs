mod schema;

use besu::{
    operators::{eq, WhereCondition},
    queries::Query,
    FromValue, LeftJoin, SqlValue,
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

#[tokio::test]
async fn values() {
    assert_eq!(
        <u64 as FromValue>::from_value(vec![SqlValue::Integer(5i64)]),
        Ok(5u64)
    );

    // TODO: More tests
}

#[track_caller]
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

    {
        let query: Query<User> = db.select(users);
        assert_sql(
            query.to_sql(),
            "SELECT users.id, users.name, users.age, users.bio, users.phone FROM users",
            vec![],
        );
        let result: Vec<User> = query.await.unwrap();
        println!("{result:?}"); // TODO
        assert_eq!(result.len(), 3); // TODO: Actually assert some values.
    }

    {
        let query: Query<User> = db.select(users)._where(eq(users::id, 0u64));
        assert_sql(
            query.to_sql(),
            "SELECT users.id, users.name, users.age, users.bio, users.phone FROM users WHERE users.id = 0",
            vec![],
        );
        let result: Vec<User> = query.await.unwrap();
        println!("{result:?}"); // TODO
        assert_eq!(result.len(), 1);
        // TODO: Actually assert some values.
    }

    {
        let query: Query<User> = db
            .select(users)
            ._where(eq(users::id, 0u64))
            ._where(eq(users::name, "Monty Beaumont".to_string()));
        assert_sql(
            query.to_sql(),
            "SELECT users.id, users.name, users.age, users.bio, users.phone FROM users WHERE users.id = 0 AND users.name = 'Monty Beaumont'",
            vec![],
        );
        let result: Vec<User> = query.await.unwrap();
        println!("{result:?}"); // TODO
        assert_eq!(result.len(), 1);
        // TODO: Actually assert some values.
    }

    {
        let query: Query<LeftJoin<Post, User>> = db
            .select(posts)
            .left_join(users, eq(posts::owner_id, users::id));
        assert_sql(
            query.to_sql(),
            // TODO: Fix the column selection part of this
            "SELECT posts.id, posts.title, posts.owner_id, users.id, users.name, users.age, users.bio, users.phone FROM posts LEFT JOIN users ON posts.owner_id = users.id",
            vec![],
        );
        let result: Vec<LeftJoin<Post, User>> = query.await.unwrap();
        println!("{result:?}");
        // assert_eq!(result.len(), 1);
        // TODO: Actually assert some values.
        todo!();
    }

    // TODO: What happens if we try and do the join in reverse.
    // TODO: What if `left_join` original table.
    // TODO: `left_join` another table multiple times.

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
