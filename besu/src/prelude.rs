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
    CaseBuilder, Expr, IntoCondition, IntoExpr, IntoOrder, NullsOrder, Order, OrderClause, abs,
    and, avg, between, case, ceil, coalesce, concat, count, count_all, count_distinct, eq, floor,
    gt, gte, ilike, in_list, is_false, is_not_null, is_null, is_true, length, like, lower, lt, lte,
    max, min, ne, not, not_between, not_ilike, not_in, not_like, now, or, raw, round, sum, trim,
    upper,
};
pub use crate::macros::Fragment;
pub use crate::query::{Delete, Insert, JoinKind, OnConflict, Select, Update};
pub use crate::selection::Selection;
pub use crate::sql;
pub use crate::table::{Insertable, Set, Table};
