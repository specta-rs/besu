//! Besu: Typesafe query builder for Rust
// TODO: No panics, enforce docs, etc lints

mod database;
pub mod operators;
pub mod queries;
pub mod schema;
mod value;

pub use database::Database;
pub use value::SqlValue;

pub type RightJoin<T, U> = (T, Option<U>);
pub type LeftJoin<T, U> = (Option<T>, U);
pub type InnerJoin<T, U> = (T, U);
pub type FullJoin<T, U> = (Option<T>, Option<U>);

#[doc(hidden)]
pub mod internal;
