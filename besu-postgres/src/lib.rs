//! [PostgreSQL](https://www.postgresql.org) dialect and driver for [Besu](https://docs.rs/besu).
//!
//! ```no_run
//! use besu::prelude::*;
//! use besu_postgres::{Database, Sqlx};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let db = Database::new(Sqlx::new("postgres://user:password@localhost/app").await?);
//! # Ok(())
//! # }
//! ```
#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]

mod ddl;
mod dialect;
#[cfg(feature = "sqlx")]
mod introspect;
#[cfg(feature = "sqlx")]
mod sqlx;

pub use ddl::PostgresDdl;
pub use dialect::Postgres;
#[cfg(feature = "sqlx")]
pub use introspect::introspect;
#[cfg(feature = "sqlx")]
pub use sqlx::Sqlx;

/// A [`besu::Database`] backed by PostgreSQL.
#[cfg(feature = "sqlx")]
pub type Database = besu::Database<Sqlx>;
