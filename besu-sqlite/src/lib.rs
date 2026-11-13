//! [SQLite](https://www.sqlite.org) dialect and drivers for [Besu](https://docs.rs/besu).
//!
//! The dialect and the DDL are always available. Execution comes in two flavours:
//!
//! - `Native`, behind the default `native` feature, is `rusqlite` on a dedicated
//!   connection thread. It has no sqlx in its path, a single connection instead of a pool,
//!   and `:memory:` works because that connection is the only one.
//! - `Sqlx`, behind the optional `sqlx` feature, is a `sqlx::SqlitePool`. Choose it when
//!   you want a pool or an existing `sqlx::SqlitePool`; while the feature is on, `Database`
//!   names this driver.
//!
//! ```no_run
//! # #[cfg(feature = "native")]
//! # {
//! use besu::prelude::*;
//! use besu_sqlite::{Native, NativeDatabase};
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // The constructors are synchronous: the connection is opened and moved to its thread.
//! let db: NativeDatabase = Native::open("app.db")?.database();
//! # Ok(())
//! # }
//! # }
//! ```
//!
//! ```no_run
//! # #[cfg(feature = "sqlx")]
//! # {
//! use besu::prelude::*;
//! use besu_sqlite::{Database, Sqlx};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let db = Database::new(Sqlx::new("app.db").await?);
//! # Ok(())
//! # }
//! # }
//! ```
//!
//! # The native driver's shape
//!
//! `rusqlite` is synchronous, so `Native` owns a thread with the `rusqlite::Connection` and
//! sends requests to it over a channel. A query encodes its parameters once on the calling
//! side, sends owned SQL and owned values, and awaits a oneshot for the rows. The thread
//! shuts down when the last clone is dropped.
//!
//! Transactions work because every statement goes to that one connection: `begin`,
//! `commit`, `rollback` and the statements inside a transaction carry the same transaction
//! id. Nested `tx.transaction(..)` calls are savepoints, exactly as they are on the sqlx
//! driver. The cost of one connection is no pool: statements are serialised, and a driver
//! query issued while a transaction is open runs inside that transaction.
//! # What the compiler stops you doing
//!
//! These doc tests fail to compile on purpose: they pin down the guarantees that make Besu
//! worth using.
//!
//! ```compile_fail
//! use besu::prelude::*;
//! # #[derive(Table)] #[besu(table = "users")] struct User { pub id: i64, pub age: i32 }
//! # #[derive(Table)] #[besu(table = "posts")] struct Post { pub id: i64, pub title: String }
//! # fn demo(db: &besu_sqlite::Database) {
//! // `posts` was never joined into this query.
//! let _ = db.select(users).filter(posts.title.eq("hello"));
//! # }
//! ```
//!
//! ```compile_fail
//! use besu::prelude::*;
//! # #[derive(Table)] #[besu(table = "users")] struct User { pub id: i64 }
//! # #[derive(Table)] #[besu(table = "posts")] struct Post { pub id: i64, pub author_id: i64 }
//! # fn demo(db: &besu_sqlite::Database) {
//! // Both sides of a comparison are checked, not just the left one.
//! let _ = db.select(users).filter(eq(users.id, posts.author_id));
//! # }
//! ```
//!
//! ```compile_fail
//! use besu::prelude::*;
//! # #[derive(Table)] #[besu(table = "users")] struct User { pub id: i64, pub age: i32 }
//! # fn demo(db: &besu_sqlite::Database) {
//! // `age` is an integer column, so it will never compare equal to a string.
//! let _ = db.select(users).filter(users.age.eq("old"));
//! # }
//! ```
//!
//! ```compile_fail
//! use besu::prelude::*;
//! # #[derive(Table)] #[besu(table = "employees")] struct Employee { pub id: i64, pub manager_id: Option<i64> }
//! # fn demo(db: &besu_sqlite::Database) {
//! // Two aliases of one table are distinct types: only `report` was joined, so the
//! // tag-0 `manager` alias has no path in this row set even though both rename the table.
//! let manager = alias(employees, "manager");
//! let report = alias_as::<_, 1>(employees, "report");
//! let _ = db
//!     .select(report)
//!     .filter(manager.column(employees.manager_id).eq(report.column(employees.manager_id)));
//! # }
//! ```
//!
//! ```compile_fail
//! use besu::prelude::*;
//! # #[derive(Table)] #[besu(table = "users")] struct User { pub id: i64, pub age: i32 }
//! # async fn demo(db: &besu_sqlite::Database) {
//! // A delete with no filter would remove every row, so it does not compile.
//! db.delete(users).await.unwrap();
//! # }
//! ```
//!
//! ```compile_fail
//! use besu::prelude::*;
//! # #[derive(Table)] #[besu(table = "users")] struct User { #[besu(primary_key, auto_increment)] pub id: i64, pub name: String }
//! # #[derive(Table)] #[besu(table = "posts")] struct Post { #[besu(primary_key, auto_increment)] pub id: i64, pub author_id: i64 }
//! # async fn demo(db: &besu_sqlite::Database) {
//! // `insert_tree` supplies the parent's `RETURNING` projection so the result is a
//! // `User`. `.returning(..)` changes the builder's type, so it does not match.
//! let _ = db
//!     .insert_tree(
//!         db.insert(users)
//!             .value(users.insert("Alice"))
//!             .returning(users),
//!         has_many(users.id, posts.author_id),
//!         [posts.insert(0)],
//!     )
//!     .await;
//! # }
//! ```
//!
//! ```compile_fail
//! use besu::prelude::*;
//! // `sql!` counts its placeholders against its arguments at compile time.
//! let _ = besu::sql!("SELECT * FROM users WHERE id = ?");
//! ```
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

/// The suffix a table rebuild gives its scratch copy.
///
/// `ddl` creates the new table under this name and renames it into place at the end, and
/// `introspect` recognises a leftover one after a rebuild that failed part way through.
pub(crate) const REBUILD_SUFFIX: &str = "__besu_tmp";

pub use ddl::SQLiteDdl;
pub use dialect::SQLite;
#[cfg(any(feature = "sqlx", feature = "native"))]
pub use introspect::introspect;
#[cfg(feature = "native")]
pub use native::{Native, NativeError, NativeOutput, NativeRow, NativeTransaction};
#[cfg(feature = "sqlx")]
pub use sqlx::Sqlx;
#[cfg(feature = "sqlx")]
#[doc(hidden)]
pub use sqlx::arguments_for_bench;

/// A [`besu::Database`] backed by SQLite's sqlx driver.
///
/// This is `Database` whenever the `sqlx` feature is on, including when `native` is also
/// enabled. The default build has only `native`, so there `Database` names the native driver;
/// use `NativeDatabase` to name it explicitly in every build.
#[cfg(feature = "sqlx")]
pub type Database = besu::Database<Sqlx>;

/// A [`besu::Database`] backed by SQLite's native driver.
///
/// This is `Database` whenever the `sqlx` feature is off, which is the default build. With
/// both features on, `Database` is the sqlx driver and this alias is the way to name the
/// native one.
#[cfg(all(feature = "native", not(feature = "sqlx")))]
pub type Database = besu::Database<Native>;

/// A [`besu::Database`] backed by the native `rusqlite` driver.
#[cfg(feature = "native")]
pub type NativeDatabase = besu::Database<Native>;
