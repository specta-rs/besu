//! [Postgres](https://www.postgresql.org) dialect and driver for [Besu](https://docs.rs/besu).
//!
//! # Example
//! ```rust
//! use besu::{Database, Table};
//! use besu_postgres::Sqlx;
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
//!         Sqlx::new("postgres://postgres:@localhost:3306/besu")
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

pub use dialect::SQLite;
pub use sqlx::Sqlx;
