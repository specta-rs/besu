use besu::{select, sql, Database, Table};
use besu_sqlite::Sqlx;

#[derive(Debug, Table)]
// #[table(name = "bruh", schema = "schema_name")]
pub struct User {
    #[column(primary_key)]
    pub id: i32,
    pub name: String,
    pub age: i32,
    pub bio: String,
    pub phone: Option<String>,
}

#[derive(Debug, Table)]
pub struct Post {
    pub id: i32,
    pub title: String,
    pub author_id: i32,
}

select!(UserSelection, { id: User.id, user_name: User.name, post_title: Post.title }); // , bruh: User.bruh

#[tokio::main]
async fn main() {
    let db = Database::new(Sqlx::new(":memory:").await.unwrap());

    // TODO: Rename the table name to lower-case
    db.execute(sql!(
        r#"CREATE TABLE "User" (id INTEGER PRIMARY KEY, name TEXT, age INTEGER, bio TEXT, phone TEXT);"#
    ))
    .await
    .unwrap();

    // db.insert(User)
    //     .values(
    //         (0..10)
    //             .map(|i| {
    //                 User.create("Alice".into(), 20 + i, "I am Alice".into())
    //                     .phone(Some("1234567890".into()))
    //             })
    //             .collect(),
    //     )
    //     .await
    //     .unwrap();

    for i in 0..10 {
        db.execute(sql!(
            r#"INSERT INTO "User" (id, name, age, bio, phone) VALUES ($1, $2, $3, $4, $5);"#,
            i,
            // TODO: Being force to do `.to_string` is annoying. `sqlx` lacking an impl for `&str` is cringe!
            "Alice".to_string(),
            20 + i,
            "I am Alice".to_string(),
            "1234567890".to_string()
        ))
        .await
        .unwrap();
    }

    println!("{:#?}", db.select(User).await.unwrap());
    println!("{:#?}", db.select(UserSelection(User)).await.unwrap());

    // println!("{:#?}", db.select(User)._where(User.id, 5).await.unwrap());
}
