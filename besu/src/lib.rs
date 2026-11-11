//! A typesafe, async, and database-agnostic SQL query builder for Rust.
//!
//! Besu is a query builder, not an ORM. Every query you write maps onto exactly the SQL you
//! would have written by hand, so you keep full control over indexes, query plans and the
//! shape of the statement, while the compiler keeps you honest about column names, column
//! types, nullability and which tables are actually in the query.
//!
//! <section class="warning">
//!
//! Besu is in early development and is **not ready for public use!**
//!
//! </section>
//!
//! # A tour
//!
//! ```ignore
//! use besu::prelude::*;
//!
//! #[derive(Table)]
//! #[besu(table = "users")]
//! pub struct User {
//!     #[besu(primary_key, auto_increment)]
//!     pub id: i64,
//!     pub name: String,
//!     pub email: String,
//!     pub age: i32,
//!     pub bio: Option<String>,
//! }
//!
//! # async fn example(db: &besu_postgres::Database) -> Result<(), Box<dyn std::error::Error>> {
//! // SELECT "users"."id", ... FROM "users" WHERE ("users"."age" > $1)
//! let adults: Vec<User> = db
//!     .select(user)
//!     .filter(user.age.gt(18))
//!     .order_by(user.name.asc())
//!     .limit(50)
//!     .await?;
//!
//! // INSERT INTO "users" ("name", "email", "age") VALUES ($1, $2, $3) RETURNING *
//! let created: User = db
//!     .insert(user)
//!     .value(user.insert("Oscar".into(), "oscar@example.com".into(), 21))
//!     .returning(user.all())
//!     .await?
//!     .into_iter()
//!     .next()
//!     .expect("RETURNING always yields a row");
//! # Ok(())
//! # }
//! ```
//!
//! # What the compiler stops you doing
//!
//! Column types, table membership and unsafe bulk statements are all rejected at compile
//! time rather than at run time. The `besu-sqlite` crate's documentation contains doc tests
//! that pin those guarantees down.
//!
//! # Where things live
//!
//! - [`besu_core`] holds the traits and the SQL primitives. Read that first if you are
//!   writing a driver.
//! - [`sql!`](crate::sql) builds a raw, safely-parameterised statement.
//! - [`Database`] is the entry point for running queries.
//!
//! # Crates
//!
//! - [`besu-sqlite`](https://docs.rs/besu-sqlite) for [SQLite](https://www.sqlite.org).
//! - [`besu-postgres`](https://docs.rs/besu-postgres) for [PostgreSQL](https://www.postgresql.org).
//! - [`besu-mysql`](https://docs.rs/besu-mysql) for [MySQL](https://www.mysql.com).
#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]
// Builders carry `PhantomData<(&'a (), fn() -> T)>` to make their lifetimes and type
// parameters invariant without inventing a dozen marker structs; clippy reads that as
// needless complexity.
#![allow(clippy::type_complexity)]

extern crate alloc;

mod condition;
mod database;
pub mod expr;
#[cfg(feature = "tracing")]
#[cfg_attr(docsrs, doc(cfg(feature = "tracing")))]
pub mod logged;
mod macros;
#[cfg(feature = "migrate")]
pub mod migrate;
pub mod prelude;
mod query;
pub mod relation;
mod selection;
mod stream;
mod table;

pub mod types;
pub use crate::types::*;
pub use besu_core::schema::{ColumnDef, DefaultValue, ForeignKey, Index, ReferenceAction};
pub use besu_core::{
    self, Decode, Dialect, Driver, Error, Executor, Sql, TableDef, TransactionDriver, TypeDef,
    Value,
};
pub use besu_core::{RowStream, Stream, futures_stream};
#[cfg(feature = "migrate")]
pub use besu_schema as schema;
pub use condition::{Contains, Literal, LiteralPath, SameType};
pub use database::{Database, RawQuery, Transaction, dry};
#[doc(hidden)]
pub use expr::ExprKind;
pub use expr::{
    CaseBuilder, Expr, IntoCondition, IntoExpr, IntoOrder, NullsOrder, Order, OrderClause, and,
    avg, between, case, ceil, coalesce, concat, count, count_all, count_distinct, eq, floor, gt,
    gte, ilike, in_list, is_false, is_not_null, is_null, is_true, json_get, json_text, length,
    like, lower, lt, lte, matches, max, min, ne, not, not_between, not_ilike, not_in, not_like,
    now, or, raw, round, sum, trim, upper,
};
#[cfg(feature = "tracing")]
#[cfg_attr(docsrs, doc(cfg(feature = "tracing")))]
pub use logged::{Logged, LoggedTransaction, Policy, logged};
#[doc(hidden)]
pub use macros::__private;
pub use macros::Fragment;
#[cfg(feature = "migrate")]
pub use migrate::{PushResult, SchemaDriver, SchemaError};
pub use query::{
    Delete, DeleteReturning, Insert, InsertReturning, JoinKind, OnConflict, Select, Update,
    UpdateReturning,
};
pub use relation::{HasMany, WithRelation, has_many};
pub use selection::Selection;
pub use stream::Rows;
pub use table::{Insertable, Set, Table};
pub use types::{Column, NonNull, Nullability, Nullable, SqlType};

/// Derive a [`Table`] from a struct.
#[cfg(feature = "derive")]
#[cfg_attr(docsrs, doc(cfg(feature = "derive")))]
pub use besu_macros::Table;
