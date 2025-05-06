use besu::{select, sql, Database};
use besu_sqlite::Sqlx;

mod schema;

#[tokio::main]
async fn main() {
    let db = Database::new(Sqlx::new(":memory:").await.unwrap());

    let todo = db
        .select(select!({
            a: 42
        }))
        .from()
        .await;

    // TODO: Rename the table name to lower-case
    // db.execute(sql!(
    //     r#"CREATE TABLE "User" (id INTEGER PRIMARY KEY, name TEXT, age INTEGER, bio TEXT, phone TEXT);"#
    // ))
    // .await
    // .unwrap();

    // db.insert(User)
    //     .values(
    //         (0..10)
    //             .map(|i| {
    //                 User.create("Alice".into(), 20 + i, "I am Alice".into())
    //                     .phone(Some("1234567890".into()))
    //                     // TODO: Remove
    //                     .id(i)
    //             })
    //             .collect(),
    //     )
    //     .await
    //     .unwrap();

    // TODO: Should `User: IntoSql`???
    // TODO: Can we merge `DecodeRow` and `DecodeValue`???
    // TODO: What if you `db.select(42)`

    // println!("{:#?}", db.select(select!({})).await.unwrap());
    // println!("{:#?}", db.select(select!(User {})).await.unwrap());
    // println!("{:#?}", db.select(User).await.unwrap());

    // println!("{:#?}", db.select(User).await.unwrap());
    // println!("{:#?}", db.select(UserSelection(User)).await.unwrap());

    // println!("{:#?}", db.select(User)._where(User.id, 5).await.unwrap());
}
