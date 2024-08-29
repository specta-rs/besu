//! Besu: Typesafe query builder for Rust
#![cfg_attr(docsrs, feature(doc_cfg))]

mod column;
mod database;
mod dialect;
mod driver;
mod error;
mod model;
mod operations;
mod table;

#[cfg(feature = "derive")]
#[cfg_attr(docsrs, doc(cfg(feature = "derive")))]
pub use besu_macros::Table;
pub use column::Column;
pub use database::Database;
pub use dialect::Dialect;
pub use driver::Driver;
pub use error::Error;
pub use model::Model;
pub use operations::*;
pub use table::Table;

pub trait Bruh<D: Driver> {
    fn decode(row: D::Row) -> Self;
}

pub trait Bruh2<D: Driver> {
    fn decode<'a>(value: D::Value<'a>) -> Self;
}
