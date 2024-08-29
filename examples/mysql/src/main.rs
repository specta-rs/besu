use std::marker::PhantomData;

use besu::{Database, Table};
use besu_mysql::Sqlx;

// TODO: DB Migrations? - primary keys, default column values

#[derive(Debug, Table)]
pub struct User {
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

// selection!(UserBase, {
//     id: User.id,
//     name: User.name,
//     age: User.age,
// });

// selection!(UserWithPosts, {
//     id: Post.id,
//     title: Post.name
//     authorId: Post.author_id,
//     authorName: User.name,
// });

#[tokio::main]
async fn main() {
    // let mysql = MySql::new().await
    // let db = Database::new().await.unwrap();

    // "mysql://root:@localhost:3306/mattrax"

    // TODO: Run migrations

    // select(User).from(User);
    // select(UserWithPosts).from(User);

    // select(User);
    // select_from(UserWithPosts, User);
}

fn select<T>(t: T) -> SelectQuery<T> {
    todo!();
}

pub struct SelectQuery<T> {
    phantom: PhantomData<T>,
}

impl<T> SelectQuery<T> {
    pub fn from<M>(self, t: M) {}
}
