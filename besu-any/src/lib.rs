//! A Besu database whose backend is chosen at runtime.
//!
//! Besu's typed query builders are compile-time over a dialect, and that is deliberate: the SQL
//! a query produces depends on the dialect, so a query cannot be shared between two databases
//! whose SQL differs. What *can* be dynamic is everything around them — connecting to whatever
//! `DATABASE_URL` points at, applying a schema, running migrations, and running a statement
//! whose SQL is already text.
//!
//! [`AnyDatabase`] gives you exactly that, and nothing more:
//!
//! ```no_run
//! # async fn example(schema: &besu::schema::Schema) -> Result<(), Box<dyn std::error::Error>> {
//! use besu_any::AnyDatabase;
//!
//! let db = AnyDatabase::connect(&std::env::var("DATABASE_URL")?).await?;
//! db.push_schema(schema, false).await?;
//! db.migrate(&besu::schema::Migrator::from_dir("migrations")?).await?;
//! # Ok(())
//! # }
//! ```
//!
//! For typed queries, match on the variant and use the concrete database:
//!
//! ```ignore
//! match db {
//!     AnyDatabase::Sqlite(db) => list_users(db).await,
//!     AnyDatabase::Postgres(db) => list_users(db).await,
//!     AnyDatabase::MySql(db) => list_users(db).await,
//! }
//!
//! async fn list_users<Dr: besu::Driver>(db: &besu::Database<Dr>) -> Result<Vec<User>, Dr::Error>
//! where
//!     User: besu::Decode<Dr>,
//! {
//!     db.select(user).await
//! }
//! ```
//!
//! A generic helper like that is the answer to "I want one function body for every backend": the
//! dialect lives in the type, and the type comes back as soon as you name a variant.
#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]

mod database;

pub use database::{AnyDatabase, AnyOutput, Backend};
