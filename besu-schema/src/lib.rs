//! Schema introspection, diffing, DDL generation and migrations for [Besu](https://docs.rs/besu).
//!
//! The unifying idea is a single, serialisable [`Schema`] IR. Three things produce it and
//! three things consume it:
//!
//! ```text
//! #[derive(Table)]  ─┐
//! live database     ─┼──▶ Schema ──▶ diff ──▶ DDL ──▶ database
//! snapshot on disk  ─┘
//! ```
//!
//! Because the desired schema and the live schema are the same type, `db push` is simply
//! "introspect, diff, apply", and because the IR is `Serialize`, snapshots and migration
//! plans fall out for free.
#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]

/// Dialect-specific DDL generation.
pub mod ddl;
/// Schema diffing.
pub mod diff;
/// The schema intermediate representation.
pub mod ir;
/// Applying migration files.
pub mod migrate;

pub use ddl::Ddl;
pub use diff::{Change, Safety, diff_with};
pub use ir::{
    Check, Column, Default, Enum, ForeignKey, Generated, Index, IndexKind, Policy, PolicyCommand,
    ReferenceAction, Schema, Sequence, Table, Unique, View, normalise_expression,
    normalise_generated_expression, normalise_view_definition, view_body,
};
pub use migrate::{AppliedMigration, Migration, MigrationError, Migrator};
