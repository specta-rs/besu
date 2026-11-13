//! [MySQL](https://www.mysql.com) dialect and drivers for [Besu](https://docs.rs/besu).
//!
//! The dialect and the DDL are always available. Execution comes in two flavours:
//!
//! - `Sqlx`, behind the default `sqlx` feature, is a `sqlx::MySqlPool`. It remains the
//!   default driver.
//! - `Native`, behind the additive `native` feature, is a `mysql_async::Pool`. It has no
//!   sqlx in its path, decodes the server's own binary values and keeps a statement cache per
//!   connection.
//!
//! ```no_run
//! # #[cfg(feature = "sqlx")]
//! # {
//! use besu::prelude::*;
//! use besu_mysql::{Database, Sqlx};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let db = Database::new(Sqlx::new("mysql://root:password@localhost/app").await?);
//! # Ok(())
//! # }
//! # }
//! ```
//!
//! ```no_run
//! # #[cfg(feature = "native")]
//! # {
//! use besu::prelude::*;
//! use besu_mysql::{Native, NativeDatabase};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let db: NativeDatabase = Native::new("mysql://root:password@localhost/app")
//!     .await?
//!     .database();
//! # Ok(())
//! # }
//! # }
//! ```
//!
//! MySQL has no `RETURNING`, so writes do not hand back the rows they touched: select them
//! afterwards, or read the number of affected rows from the result.
//!
//! # The native driver's shape
//!
//! `Native` owns a `mysql_async::Pool` of at most ten connections. A query checks one out,
//! encodes its parameters once, and returns the connection when the rows are done; concurrent
//! queries use different connections. `mysql_async` caches prepared statements per connection
//! by SQL text, so a repeated query shape skips the prepare round trip. A connection is not
//! `COM_RESET_CONNECTION`ed when it goes back to the pool — the same choice sqlx makes — so
//! the cache and the pinned session survive; a dirty transaction or an unread result is still
//! rolled back and drained first. Pass a pool built with `PoolOpts::with_reset_connection(true)`
//! to `Native::from_pool` if you want every checkout to start from a clean session.
//!
//! [`Driver::query_stream`](besu_core::Driver::query_stream) is a real stream: rows are
//! forwarded from the server as they arrive over a bounded channel, so the connection stays
//! checked out and memory stays bounded by one row. Inside a transaction the stream borrows the
//! transaction's connection instead.
//!
//! # TLS
//!
//! The native driver is compiled without a `mysql_async` TLS feature, so connections are
//! unencrypted. `mysql_async` exposes TLS as the `rustls-tls` (plus a provider such as
//! `ring`) or `native-tls-tls` features and an `SslOpts` on `Opts`. Enabling either is an API
//! decision — the driver would need to accept an `Opts`/`SslOpts` or expose a constructor that
//! does — so it is deliberately left to its own change. A URL that asks for TLS
//! (`require_ssl=true`) hits `mysql_async`'s own guard, which panics with a message naming the
//! features to enable; it never silently goes unencrypted. An application that enables a
//! `mysql_async` TLS feature itself can still pass a pool built with `SslOpts` to
//! `Native::from_pool`.
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

pub use ddl::MySqlDdl;
pub use dialect::MySql;
#[cfg(any(feature = "sqlx", feature = "native"))]
pub use introspect::introspect;
#[cfg(feature = "native")]
pub use native::{Native, NativeError, NativeOutput, NativeRow, NativeTransaction};
#[cfg(feature = "sqlx")]
pub use sqlx::Sqlx;
#[cfg(feature = "sqlx")]
#[doc(hidden)]
pub use sqlx::arguments_for_bench;

/// A [`besu::Database`] backed by MySQL's sqlx driver.
///
/// This is the default whenever the `sqlx` feature is on, including when `native` is also
/// enabled. Use `NativeDatabase` to name the native driver explicitly.
#[cfg(feature = "sqlx")]
pub type Database = besu::Database<Sqlx>;

/// A [`besu::Database`] backed by MySQL's native driver.
///
/// This is `Database` only when `native` is enabled without `sqlx`; with both features on,
/// `Database` is the sqlx driver and this alias is the way to name the native one.
#[cfg(all(feature = "native", not(feature = "sqlx")))]
pub type Database = besu::Database<Native>;

/// A [`besu::Database`] backed by the native `mysql_async` driver.
#[cfg(feature = "native")]
pub type NativeDatabase = besu::Database<Native>;
