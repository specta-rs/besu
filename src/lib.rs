//! Besu: Typesafe query builder for Rust
// TODO: No panics, enforce docs, etc lints

mod database;
mod error;
mod executed_query;
mod from_value;
mod insert;
pub mod operators;
pub mod queries;
pub mod schema;
mod value;

pub use besu_macros::table;
pub use database::Database;
pub use error::Error;
pub use executed_query::ExecutedQuery;
pub use from_value::FromValue;
pub use internal::Contains;
pub use value::SqlValue;

pub type RightJoin<T, U> = (T, Option<U>);
pub type LeftJoin<T, U> = (Option<T>, U);
pub type InnerJoin<T, U> = (T, U);
pub type FullJoin<T, U> = (Option<T>, Option<U>);

#[doc(hidden)]
pub mod internal;
