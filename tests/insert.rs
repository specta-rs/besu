use besu::Database;

pub use schema::User;

mod schema;

// INSERT INTO table_name(id, name, etc) VALUES (value1, value2, value3, ...);

#[tokio::test]
async fn insert() {
    let db = Database::new(":memory:").await.unwrap();

    db.insert()
        .value(User::insert {
            name: "Monty Beaumont".into(),
            password: None,
        })
        .await
        .unwrap();

    db.insert()
        .value(User::insert("Oscar Beaumont").password("hunter2"))
        .await
        .unwrap();

    let mut new_user = User::insert("Millie Beaumont");

    // Don't do
    new_user = new_user.password("hunter2");

    // Do this instead
    new_user.edit().password("hunter4");

    db.insert().value(new_user).await.unwrap();

    // TODO: Insert and link with relation

    // TODO: Insert and create relation at same time???
}

#[tokio::test]
async fn insert_many() {
    let db = Database::new(":memory:").await.unwrap();

    db.insert().values(&[User::insert("George")]).await.unwrap();

    db.insert()
        .values(vec![User::insert("George")])
        .await
        .unwrap();
}

// TODO: https://orm.drizzle.team/docs/insert#upserts-and-conflicts

// TODO: https://orm.drizzle.team/docs/insert#insert-returning
