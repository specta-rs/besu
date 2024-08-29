//! besu-mysql: SQLite dialect and driver for Besu
#![cfg_attr(docsrs, feature(doc_cfg))]

mod dialect;
mod sqlx;

pub use dialect::MySQL;
pub use sqlx::Sqlx;
