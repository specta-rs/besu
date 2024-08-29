//! besu-mysql: SQLite dialect and driver for Besu
#![cfg_attr(docsrs, feature(doc_cfg))]

mod dialect;
mod encode;
mod sqlx;

pub use dialect::SQLite;
pub use encode::SqliteArgument;
pub use sqlx::Sqlx;
