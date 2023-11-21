pub use schema::User;

use crate::schema::users;

mod schema;

// INSERT INTO table_name(id, name, etc) VALUES (value1, value2, value3, ...);

#[tokio::test]
async fn insert_struct_syntax() {
    let db = schema::init().await;

    let query = db.insert().value(
        users::insert {
            name: "Oscar Beaumont".into(),
            age: 21,
            bio: Some("Rust, rust, rust".into()),
            phone: None,
        }
        // Set optional fields or update other fields
        .bio(Some("My new bio".into())),
    );
    assert_eq!(query.to_sql().unwrap().0, "INSERT INTO users(name, age, bio, phone) VALUES ('Oscar Beaumont', 21, 'My new bio', NULL)");
    query.await.unwrap();
    // TODO: Assert it was inserted correctly

    let mut user = users::insert {
        name: "Oscar Beaumont".into(),
        age: 21,
        bio: Some("Rust, rust, rust".into()),
        phone: None,
    };

    // Use `edit()` to update fields (`&mut self` instead of `self`). You can chain updates!
    user.edit().bio(Some("My new bio".into())).age(21);

    let query = db.insert().value(user);
    assert_eq!(query.to_sql().unwrap().0, "INSERT INTO users(name, age, bio, phone) VALUES ('Oscar Beaumont', 21, 'My new bio', NULL)");
    query.await.unwrap();
    // TODO: Assert it was inserted correctly
}

#[tokio::test]
async fn insert_function_syntax() {
    let db = schema::init().await;

    let query = db.insert().value(
        User::insert("Oscar Beaumont", 21, Some("Rust, rust, rust".into()), None)
            .bio(Some("My new bio".into())),
    );
    assert_eq!(query.to_sql().unwrap().0, "INSERT INTO users(name, age, bio, phone) VALUES ('Oscar Beaumont', 21, 'My new bio', NULL)");
    query.await.unwrap();

    let mut user = User::insert("Oscar Beaumont", 21, Some("Rust, rust, rust".into()), None);

    // Use `edit()` to update fields (`&mut self` instead of `self`). You can chain updates!
    user.edit().bio(Some("My new bio".into())).age(21);

    let query = db.insert().value(user);
    assert_eq!(query.to_sql().unwrap().0, "INSERT INTO users(name, age, bio, phone) VALUES ('Oscar Beaumont', 21, 'My new bio', NULL)");
    query.await.unwrap();
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
            name: "Oscar Beaumont".into(),
            age: 21,
            bio: Some("Rust, rust, rust".into()),
            phone: None,
        },
        users::insert {
            name: "Oscar Beaumont".into(),
            age: 21,
            bio: Some("Rust, rust, rust".into()),
            phone: None,
        },
    ]);
    assert_eq!(query.to_sql().unwrap().0, "INSERT INTO users(name, age, bio, phone) VALUES ('Oscar Beaumont', 21, 'Rust, rust, rust', NULL), ('Oscar Beaumont', 21, 'Rust, rust, rust', NULL)");
    query.await.unwrap();
    // TODO: Assert they were inserted correctly
}

// TODO: Some basic unit tests for SQL injection just to be sure

// TODO: https://orm.drizzle.team/docs/insert#upserts-and-conflicts

// TODO: https://orm.drizzle.team/docs/insert#insert-returning
