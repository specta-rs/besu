//! TODO
#![allow(dead_code)]

// #[derive(Debug, Table)]
// // #[table(name = "bruh", schema = "schema_name")]
// pub struct User {
//     #[column(primary_key)]
//     pub id: i32,
//     pub name: String,
//     pub age: i32,
//     pub bio: String,
//     pub phone: Option<String>,
// }

// #[derive(Debug, Table)]
// pub struct Post {
//     pub id: i32,
//     pub title: String,
//     pub author_id: i32,
// }

pub mod users {
    use besu::schema::Column;

    pub(super) struct Model {
        pub id: Column<id::Type>,
    }

    pub mod id {
        pub type Type = String;
    }
}
use besu::schema::Column;

#[allow(non_upper_case_globals, private_interfaces)]
pub const users: users::Model = users::Model {
    id: Column::new("'users.id'"), // TODO: Don't require escaping
};
