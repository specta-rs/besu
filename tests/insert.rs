use besu::SqlValue;
pub use schema::User;

use crate::schema::{assert_sql, users};

mod schema;

// INSERT INTO table_name(id, name, etc) VALUES (value1, value2, value3, ...);

#[tokio::test]
async fn insert_struct_syntax() {
    let db = schema::init().await;

    {
        let query = db.insert().value(
            users::insert {
                id: None,
                name: "Oscar Beaumont".into(),
                age: 21,
                bio: Some("Rust, rust, rust".into()),
                phone: None,
            }
            // Set optional fields or update other fields
            .bio("My new bio"),
        );
        assert_sql(
            query.to_sql(),
            "INSERT INTO users(name, age, bio, phone) VALUES (?, ?, ?, ?)",
            vec![
                "Oscar Beaumont".into(),
                21.into(),
                "My new bio".into(),
                SqlValue::Null,
            ],
        );
        query.await.unwrap();
        // TODO: Assert it was inserted correctly
    }

    {
        let mut user = users::insert {
            id: None,
            name: "Oscar Beaumont".into(),
            age: 21,
            bio: Some("Rust, rust, rust".into()),
            phone: None,
        };

        // Use `edit()` to update fields (`&mut self` instead of `self`). You can chain updates!
        user.edit().bio("My new bio").age(21u64);

        let query = db.insert().value(user);
        assert_sql(
            query.to_sql(),
            "INSERT INTO users(name, age, bio, phone) VALUES (?, ?, ?, ?)",
            vec![
                "Oscar Beaumont".into(),
                21.into(),
                "My new bio".into(),
                SqlValue::Null,
            ],
        );
        query.await.unwrap();
        // TODO: Assert it was inserted correctly
    }
}

#[tokio::test]
async fn insert_function_syntax() {
    let db = schema::init().await;

    {
        let query = db
            .insert()
            .value(users::insert("Oscar Beaumont", 21u64).bio("My new bio"));
        assert_sql(
            query.to_sql(),
            "INSERT INTO users(name, age, bio, phone) VALUES (?, ?, ?, ?)",
            vec![
                "Oscar Beaumont".into(),
                21.into(),
                "My new bio".into(),
                SqlValue::Null,
            ],
        );
        query.await.unwrap();
    }

    {
        let mut user = users::insert("Oscar Beaumont", 21u64);

        // Use `edit()` to update fields (`&mut self` instead of `self`). You can chain updates!
        user.edit().bio("My new bio").age(21u64);

        let query = db.insert().value(user);
        assert_sql(
            query.to_sql(),
            "INSERT INTO users(name, age, bio, phone) VALUES (?, ?, ?, ?)",
            vec![
                "Oscar Beaumont".into(),
                21.into(),
                "My new bio".into(),
                SqlValue::Null,
            ],
        );
        query.await.unwrap();
    }
}

#[tokio::test]
async fn insert_many() {
    let db = schema::init().await;

    // TODO: This is a panic. Assert panic
    // let query = db.insert().values([] as [users::insert; 0]);
    // assert_eq!(query.to_sql().unwrap().0, "");
    // query.await.unwrap();

    let query = db.insert().values([
        users::insert {
            id: None,
            name: "Oscar Beaumont".into(),
            age: 21,
            bio: Some("Rust, rust, rust".into()),
            phone: None,
        },
        users::insert {
            id: None,
            name: "Oscar Beaumont".into(),
            age: 21,
            bio: Some("Rust, rust, rust".into()),
            phone: None,
        },
    ]);
    assert_sql(
        query.to_sql(),
        "INSERT INTO users(name, age, bio, phone) VALUES (?, ?, ?, ?), (?, ?, ?, ?)",
        vec![
            "Oscar Beaumont".into(),
            21.into(),
            "Rust, rust, rust".into(),
            SqlValue::Null,
            "Oscar Beaumont".into(),
            21.into(),
            "Rust, rust, rust".into(),
            SqlValue::Null,
        ],
    );
    query.await.unwrap();
    // TODO: Assert they were inserted correctly
}

// TODO: Some basic unit tests for SQL injection just to be sure

// TODO: https://orm.drizzle.team/docs/insert#upserts-and-conflicts

// TODO: https://orm.drizzle.team/docs/insert#insert-returning
