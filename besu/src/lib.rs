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

mod arguments;
mod column;
mod database;
mod decode;
mod dialect;
mod driver;
mod macros;
mod operations;
mod operator;
mod table;

pub use arguments::{Arguments, EncodeArgument};
#[cfg(feature = "derive")]
#[cfg_attr(docsrs, doc(cfg(feature = "derive")))]
pub use besu_macros::Table;
pub use column::Column;
pub use database::Database;
pub use decode::{DecodeRow, DecodeValue};
pub use dialect::Dialect;
pub use driver::Driver;
pub use operations::*;
pub use table::Table;
