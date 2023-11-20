pub use schema::User;

use crate::schema::users;

mod schema;

// INSERT INTO table_name(id, name, etc) VALUES (value1, value2, value3, ...);

#[tokio::test]
async fn insert_struct_syntax() {
    let db = schema::init().await;

    db.insert()
        .value(
            users::insert {
                name: "Oscar Beaumont".into(),
                age: 21,
                bio: Some("Rust, rust, rust".into()),
                phone: None,
            }
            // Set optional fields or update other fields
            .bio(Some("My new bio".into())),
        )
        .await
        .unwrap();
    // TODO: Assert it was inserted correctly

    let mut user = users::insert {
        name: "Oscar Beaumont".into(),
        age: 21,
        bio: Some("Rust, rust, rust".into()),
        phone: None,
    };

    // Use `edit()` to update fields (`&mut self` instead of `self`). You can chain updates!
    user.edit().bio(Some("My new bio".into())).age(21);

    db.insert().value(user).await.unwrap();
    // TODO: Assert it was inserted correctly
}

#[tokio::test]
async fn insert_function_syntax() {
    let db = schema::init().await;

    db.insert()
        .value(
            User::insert("Oscar Beaumont", 21, Some("Rust, rust, rust".into()), None)
                .bio(Some("My new bio".into())),
        )
        .await
        .unwrap();

    let mut user = User::insert("Oscar Beaumont", 21, Some("Rust, rust, rust".into()), None);

    // Use `edit()` to update fields (`&mut self` instead of `self`). You can chain updates!
    user.edit().bio(Some("My new bio".into())).age(21);

    db.insert().value(user).await.unwrap();
}

#[tokio::test]
async fn insert_many() {
    let db = schema::init().await;

    db.insert().values([] as [users::insert; 0]).await.unwrap();

    db.insert()
        .values([
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
        ])
        .await
        .unwrap();
    // TODO: Assert they were inserted correctly
}

// TODO: https://orm.drizzle.team/docs/insert#upserts-and-conflicts

// TODO: https://orm.drizzle.team/docs/insert#insert-returning
