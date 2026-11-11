//! Everything you need in scope, in one import.
//!
//! ```ignore
//! use besu::prelude::*;
//! ```
//!
//! This brings in the query builders, the expression helpers, the derive macro and the
//! traits that make method syntax work.

pub use crate::types::{Column, NonNull, Nullability, Nullable, SqlType};
pub use besu_core::{TypeDef, Value};

#[cfg(feature = "derive")]
pub use besu_macros::Table;

pub use crate::database::{Database, RawQuery, Transaction, dry};
pub use crate::expr::{
    CaseBuilder, Expr, IntoCondition, IntoExpr, IntoOrder, IntoSubquery, NullsOrder, Order,
    OrderClause, abs, and, avg, between, case, ceil, coalesce, concat, count, count_all,
    count_distinct, eq, exists, floor, gt, gte, ilike, in_list, is_false, is_not_null, is_null,
    is_true, json_get, json_text, length, like, lower, lt, lte, matches, max, min, ne, not,
    not_between, not_exists, not_ilike, not_in, not_like, now, or, raw, round, sum, trim, upper,
};
pub use crate::macros::Fragment;
pub use crate::query::{Delete, Insert, JoinKind, OnConflict, Select, SetOp, SetQuery, Update};
pub use crate::relation::{HasMany, WithRelation, has_many};
pub use crate::selection::Selection;
pub use crate::sql;
pub use crate::table::{Insertable, Set, Table};
