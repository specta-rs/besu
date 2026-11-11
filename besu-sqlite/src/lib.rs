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
#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]

mod dialect;
#[cfg(feature = "sqlx")]
mod sqlx;

pub use dialect::SQLite;
#[cfg(feature = "sqlx")]
pub use sqlx::Sqlx;

/// A [`besu::Database`] backed by SQLite.
#[cfg(feature = "sqlx")]
pub type Database = besu::Database<Sqlx>;
