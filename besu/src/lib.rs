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

mod batch;
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
pub mod seed;
mod selection;
mod stream;
mod table;

pub mod types;
pub use crate::types::*;
pub use batch::{Batch, BatchItem, BatchWrite};
pub use besu_core::schema::{
    CheckDef, ColumnDef, DefaultValue, ForeignKey, Generated, Index, IndexColumn, IndexEntry,
    IndexKind, ReferenceAction,
};
pub use besu_core::{
    self, Decode, Dialect, DomainDef, Driver, EnumDef, EnumLabels, Error, Executor, Sql, TableDef,
    TransactionDriver, TypeDef, Value,
};
pub use besu_core::{RowStream, Stream, futures_stream};
#[cfg(feature = "migrate")]
pub use besu_schema as schema;
pub use condition::{Contains, Literal, LiteralPath, SameType};
pub use database::{Database, RawQuery, Transaction, dry};
#[doc(hidden)]
pub use expr::ExprKind;
pub use expr::{
    AvgAs, CaseBuilder, DateField, Expr, FrameBound, FrameUnits, Interval, IntoCondition, IntoExpr,
    IntoOrder, IntoSqlValue, IntoSubquery, JsonOperator, JsonPath, JsonRight, NullsOrder, Order,
    OrderClause, SumAs, Window, and, array_agg, array_append, array_cat, array_contained_by,
    array_contains, array_get, array_length, array_overlaps, array_position, array_prepend,
    array_remove, array_replace, array_to_string, avg, avg_as, between, bool_and, bool_or, btrim,
    cardinality, case, ceil, char_length, coalesce, coalesce3, coalesce4, coalesce5, concat,
    concat_ws, concat_ws4, concat3, concat4, concat5, concat6, concat7, concat8, contains_json,
    count, count_all, count_distinct, current_date, date_add, date_format, date_sub, date_trunc,
    datediff, dense_rank, eq, every, excluded, exists, exp, extract, floor, from_base64,
    from_unixtime, greatest, gt, gte, ilike, in_list, initcap, is_distinct_from, is_false,
    is_not_distinct_from, is_not_null, is_null, is_true, json_agg, json_array_agg, json_at,
    json_at_text, json_contained_by, json_get, json_has_all, json_has_any, json_has_key, json_key,
    json_key_text, json_matches, json_matches_path, json_merge_patch, json_object, json_object_agg,
    json_overlaps, json_query, json_remove, json_set, json_strip, json_text, json_value, jsonb_agg,
    lag, lag_by, lead, lead_by, least, left, length, like, ln, log, log10, lower, lpad, lt, lte,
    ltrim, matches, max, md5, min, ne, new_row, not, not_between, not_exists, not_ilike, not_in,
    not_like, not_regex_match, not_regex_match_ci, now, nullif, octet_length, or, power, rank, raw,
    regex_match, regex_match_ci, regexp_replace, regexp_substr, repeat, replace, reverse, right,
    round, round_to, row_number, rpad, rtrim, sha256, sign, split_part, sqrt, stddev, stddev_pop,
    string_agg, string_to_array, strpos, substr, substring, sum, sum_as, to_base64, translate,
    trim, trunc, unix_timestamp, upper, var_pop, variance,
};
#[cfg(feature = "tracing")]
#[cfg_attr(docsrs, doc(cfg(feature = "tracing")))]
pub use logged::{Logged, LoggedTransaction, Policy, logged};
#[doc(hidden)]
pub use macros::__private;
pub use macros::Fragment;
#[cfg(feature = "migrate")]
pub use migrate::{PushResult, SchemaDriver, SchemaError};
#[doc(hidden)]
pub use query::QueryBody;
pub use query::{
    Alias, Cte, CteColumns, Delete, DeleteReturning, Derived, DynamicSelect, Insert,
    InsertReturning, IntoQueryBody, JoinKind, OnConflict, Select, SetOp, SetQuery, Update,
    UpdateReturning, alias, alias_as, derived,
};
pub use relation::{HasMany, WithRelation, has_many};
pub use seed::{Seed, SeedError, SeedReport, TableSeed, seed};
pub use selection::Selection;
pub use stream::Rows;
pub use table::{Insertable, Set, Table, TableRef};
pub use types::{
    Column, NonNull, Nullability, Nullable, SqlEnum, SqlType, TypeWitness, UnknownLabel,
};

/// Derive a [`Table`] from a struct.
#[cfg(feature = "derive")]
#[cfg_attr(docsrs, doc(cfg(feature = "derive")))]
pub use besu_macros::Table;

/// Derive [`SqlEnum`] from an enum, so its variants become a SQL enum's labels.
#[cfg(feature = "derive")]
#[cfg_attr(docsrs, doc(cfg(feature = "derive")))]
pub use besu_macros::SqlEnum;
