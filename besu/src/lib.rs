//! A typesafe, async, and database-agnostic query builder for Rust.
//!
//! <section class="warning">
//!
//! Besu is in early development and is **not ready for public use!**
//!
//! </section>
//!
//! ## Drivers/Dialects
//!
//! - [besu-mysql](https://docs.rs/besu-mysql) for [MySQL](https://www.mysql.com) support.
//! - [besu-sqlite](https://docs.rs/besu-sqlite) for [SQLite](https://www.sqlite.org) support.
//! - [besu-postgres](https://docs.rs/besu-postgres) for [PostgreSQL](https://www.postgresql.org) support.
//!
#![cfg_attr(docsrs, feature(doc_cfg))]

mod database;
mod dialect;
mod driver;
pub mod operations;
pub mod schema;
mod sql;

pub use database::Database;
pub use dialect::Dialect;
pub use driver::Driver;
pub use sql::Sql;
