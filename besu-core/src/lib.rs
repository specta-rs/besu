//! Core traits and SQL primitives for [Besu](https://docs.rs/besu).
//!
//! This crate is deliberately dependency-free and knows nothing about how a query is
//! executed. It defines the two halves of Besu's explicit interface:
//!
//! - [`Dialect`] — everything about how a specific SQL database *spells* things. A dialect
//!   can render a complete query (SQL text plus bound values) with no database connection
//!   in sight, which is what makes [`Sql`] driverless.
//! - [`Driver`] — everything about how a specific database *executes* things: running a
//!   [`Sql`] and handing back rows.
//!
//! Everything else (the query builders, the schema definition macros) lives in `besu` and
//! compiles down to these primitives.
//!
//! <section class="warning">
//!
//! Besu is in early development and is **not ready for public use!**
//!
//! </section>
#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]

extern crate alloc;

/// Row decoding.
pub mod decode;
/// SQL dialects: how a specific database spells things.
pub mod dialect;
/// Running SQL against a database.
pub mod driver;
/// Errors produced while building or running a query.
pub mod error;
/// Where queries run: a pool, a transaction, or nowhere at all.
pub mod executor;
/// Static table and column metadata.
pub mod schema;
/// A rendered SQL statement with its bound parameters.
pub mod sql;
/// SQL column types expressed as Rust types.
pub mod types;
/// An owned, dialect-independent SQL value.
pub mod value;

pub use decode::Decode;
pub use dialect::{Dialect, JsonPathStyle, Placeholder, UpsertStyle};
pub use driver::{Driver, RowStream, TransactionDriver, futures_stream};
pub use error::Error;
pub use executor::{DialectOf, Dry, DryDriver, DryRow, ErrorOf, Executor, RowOf};
pub use futures_core::Stream;
pub use schema::{ColumnDef, DefaultValue, ForeignKey, Generated, Index, IndexKind, TableDef};
pub use sql::{Rendered, Sql};
pub use types::TypeDef;
pub use value::Value;
