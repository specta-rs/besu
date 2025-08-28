//! A typesafe, async, and database-agnostic query builder for Rust.
//!
//! <section class="warning">
//!
//! Besu is in early development and is **not ready for public use!**
//!
//! </section>
//!
//! ## Drivers
//!
//! - [besu-mysql](https://docs.rs/besu-mysql) for [MySQL](https://www.mysql.com) support.
//! - [besu-sqlite](https://docs.rs/besu-sqlite) for [SQLite](https://www.sqlite.org) support.
//! - [besu-postgres](https://docs.rs/besu-postgres) for [PostgreSQL](https://www.postgresql.org) support.
//!
#![cfg_attr(docsrs, feature(doc_cfg))]

mod db;
mod driver;
mod into_sql;
mod join;
mod mysql;
mod sql;

pub use db::Db;
pub use driver::{Argument, Driver};
pub use into_sql::IntoSql;
pub use join::Join;
pub use mysql::MySql;
pub use sql::{identifier, null, sql, unsafe_sql, value, Arguments, Never, Sql};
