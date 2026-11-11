//! [MySQL](https://www.mysql.com) dialect and driver for [Besu](https://docs.rs/besu).
//!
//! ```no_run
//! use besu::prelude::*;
//! use besu_mysql::{Database, Sqlx};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let db = Database::new(Sqlx::new("mysql://root:password@localhost/app").await?);
//! # Ok(())
//! # }
//! ```
//!
//! MySQL has no `RETURNING`, so writes do not hand back the rows they touched: select them
//! afterwards, or read the number of affected rows from the result.
#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]

mod ddl;
mod dialect;
#[cfg(feature = "sqlx")]
mod introspect;
#[cfg(feature = "sqlx")]
mod sqlx;

pub use ddl::MySqlDdl;
pub use dialect::MySql;
#[cfg(feature = "sqlx")]
pub use introspect::introspect;
#[cfg(feature = "sqlx")]
pub use sqlx::Sqlx;
#[cfg(feature = "sqlx")]
#[doc(hidden)]
pub use sqlx::arguments_for_bench;

/// A [`besu::Database`] backed by MySQL.
#[cfg(feature = "sqlx")]
pub type Database = besu::Database<Sqlx>;
