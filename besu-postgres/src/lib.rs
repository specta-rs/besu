//! [PostgreSQL](https://www.postgresql.org) dialect and drivers for [Besu](https://docs.rs/besu).
//!
//! The dialect and the DDL are always available. Execution comes in two flavours:
//!
//! - `Native`, behind the default `native` feature, is `tokio-postgres` over a
//!   `deadpool-postgres` pool. It has no sqlx in its path and encodes parameters in the
//!   binary format of whatever type the server infers.
//! - `Sqlx`, behind the optional `sqlx` feature, is a `sqlx::PgPool`. Choose it for TLS or
//!   an existing `sqlx::PgPool`; while the feature is on, `Database` names this driver.
//!
//! ```no_run
//! # #[cfg(feature = "native")]
//! # {
//! use besu::prelude::*;
//! use besu_postgres::{Native, NativeDatabase};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let db: NativeDatabase = Native::new("postgres://user:password@localhost/app")
//!     .await?
//!     .database();
//! # Ok(())
//! # }
//! # }
//! ```
//!
//! ```no_run
//! # #[cfg(feature = "sqlx")]
//! # {
//! use besu::prelude::*;
//! use besu_postgres::{Database, Sqlx};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let db = Database::new(Sqlx::new("postgres://user:password@localhost/app").await?);
//! # Ok(())
//! # }
//! # }
//! ```
//!
//! # The native driver's shape
//!
//! `Native` owns a `deadpool_postgres::Pool` of at most ten connections. A query checks one
//! out, encodes its parameters once, and returns it when the rows are done; concurrent
//! queries use different connections, so nothing is serialised behind a mutex. A statement
//! is prepared once and cached by `tokio-postgres` for the connection that runs it.
//!
//! The SQL text does not declare parameter types, so the server infers one per placeholder
//! from the column or the cast around it. The encoder writes the value in exactly that type:
//! a `Value::Int` bound to an `INT2`, `INT4`, `INT8` or `NUMERIC` column is written two,
//! four, eight or variable-width bytes respectively. `NULL`, enum labels, arrays, ranges,
//! geometry and vectors are sent in the form the inferred type expects, with no explicit
//! `TEXT` declaration that an enum column would reject.
//!
//! [`Driver::query_stream`](besu_core::Driver::query_stream) is a real stream: the pooled
//! connection stays checked out until the stream is dropped. Inside a transaction the stream
//! borrows the transaction's connection instead.
//!
//! # TLS
//!
//! The native driver uses `tokio_postgres::NoTls`. Exposing TLS means a feature that pulls
//! in `rustls` or `native-tls`, a connector argument on the constructors, and a `sslmode`
//! story; those are API decisions rather than implementation details, so they are left to a
//! later change. A URL that requires TLS fails to connect instead of silently going
//! unencrypted.
#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]

mod ddl;
mod dialect;
#[cfg(any(feature = "sqlx", feature = "native"))]
mod introspect;
#[cfg(feature = "native")]
mod native;
#[cfg(feature = "sqlx")]
mod sqlx;

pub use ddl::PostgresDdl;
pub use dialect::Postgres;
#[cfg(any(feature = "sqlx", feature = "native"))]
pub use introspect::introspect;
#[cfg(feature = "native")]
pub use native::{Native, NativeError, NativeOutput, NativeRow, NativeTransaction};
#[cfg(feature = "sqlx")]
pub use sqlx::Sqlx;

/// Encode a statement's parameters without running it.
///
/// This exists so the crate's benchmarks and allocation probes can measure the driver's
/// encoding path without a database. It is not part of the supported API and is hidden from
/// the documentation; the name may change without a major version bump.
#[cfg(feature = "sqlx")]
#[doc(hidden)]
pub use sqlx::arguments as encode_arguments;

/// A [`besu::Database`] backed by PostgreSQL's sqlx driver.
///
/// This is `Database` whenever the `sqlx` feature is on, including when `native` is also
/// enabled. The default build has only `native`, so there `Database` names the native driver;
/// use `NativeDatabase` to name it explicitly in every build.
#[cfg(feature = "sqlx")]
pub type Database = besu::Database<Sqlx>;

/// A [`besu::Database`] backed by PostgreSQL's native driver.
///
/// This is `Database` whenever the `sqlx` feature is off, which is the default build. With
/// both features on, `Database` is the sqlx driver and this alias is the way to name the
/// native one.
#[cfg(all(feature = "native", not(feature = "sqlx")))]
pub type Database = besu::Database<Native>;

/// A [`besu::Database`] backed by the native `tokio-postgres` driver.
#[cfg(feature = "native")]
pub type NativeDatabase = besu::Database<Native>;
