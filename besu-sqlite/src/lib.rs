//! [SQLite](https://www.sqlite.org) dialect and driver for [Besu](https://docs.rs/besu).
//!
//! ```no_run
//! use besu::prelude::*;
//! use besu_sqlite::{Database, Sqlx};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let db = Database::new(Sqlx::new("app.db").await?);
//! # Ok(())
//! # }
//! ```
//! # What the compiler stops you doing
//!
//! These doc tests fail to compile on purpose: they pin down the guarantees that make Besu
//! worth using.
//!
//! ```compile_fail
//! use besu::prelude::*;
//! # #[derive(Table)] #[besu(table = "users")] struct User { pub id: i64, pub age: i32 }
//! # #[derive(Table)] #[besu(table = "posts")] struct Post { pub id: i64, pub title: String }
//! # fn demo(db: &besu_sqlite::Database) {
//! // `posts` was never joined into this query.
//! let _ = db.select(users).filter(posts.title.eq("hello"));
//! # }
//! ```
//!
//! ```compile_fail
//! use besu::prelude::*;
//! # #[derive(Table)] #[besu(table = "users")] struct User { pub id: i64 }
//! # #[derive(Table)] #[besu(table = "posts")] struct Post { pub id: i64, pub author_id: i64 }
//! # fn demo(db: &besu_sqlite::Database) {
//! // Both sides of a comparison are checked, not just the left one.
//! let _ = db.select(users).filter(eq(users.id, posts.author_id));
//! # }
//! ```
//!
//! ```compile_fail
//! use besu::prelude::*;
//! # #[derive(Table)] #[besu(table = "users")] struct User { pub id: i64, pub age: i32 }
//! # fn demo(db: &besu_sqlite::Database) {
//! // `age` is an integer column, so it will never compare equal to a string.
//! let _ = db.select(users).filter(users.age.eq("old"));
//! # }
//! ```
//!
//! ```compile_fail
//! use besu::prelude::*;
//! # #[derive(Table)] #[besu(table = "users")] struct User { pub id: i64, pub age: i32 }
//! # async fn demo(db: &besu_sqlite::Database) {
//! // A delete with no filter would remove every row, so it does not compile.
//! db.delete(users).await.unwrap();
//! # }
//! ```
//!
//! ```compile_fail
//! use besu::prelude::*;
//! // `sql!` counts its placeholders against its arguments at compile time.
//! let _ = besu::sql!("SELECT * FROM users WHERE id = ?");
//! ```
#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]

mod ddl;
mod dialect;
#[cfg(feature = "sqlx")]
mod introspect;
#[cfg(feature = "sqlx")]
mod sqlx;

pub use ddl::SQLiteDdl;
pub use dialect::SQLite;
#[cfg(feature = "sqlx")]
pub use introspect::introspect;
#[cfg(feature = "sqlx")]
pub use sqlx::Sqlx;
#[cfg(feature = "sqlx")]
#[doc(hidden)]
pub use sqlx::arguments_for_bench;

/// A [`besu::Database`] backed by SQLite.
#[cfg(feature = "sqlx")]
pub type Database = besu::Database<Sqlx>;
