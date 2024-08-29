//! [SQLite](https://sqlite.org) dialect and driver for [Besu](https://docs.rs/besu).
//!
//! # Example
//! ```rust
//! use besu::{Database, Table};
//! use besu_sqlite::Sqlx;
//!
//! #[derive(Debug, Table)]
//! pub struct User {
//!     pub id: i32,
//!     pub name: String,
//! }
//!
//! #[tokio::main]
//! async fn main() {
//!     let db = Database::new(
//!         Sqlx::new(":memory:")
//!             .await
//!             .unwrap(),
//!     );
//!
//!     // TODO: Insert a user
//!
//!     println!("{:#?}", db.select(User).await.unwrap());
//! }
//! ```
#![cfg_attr(docsrs, feature(doc_cfg))]

mod dialect;
mod sqlx;

pub use dialect::Postgres;
pub use sqlx::Sqlx;
