//! Everything you need in scope, in one import.
//!
//! ```ignore
//! use besu::prelude::*;
//! ```
//!
//! This brings in the query builders, the expression helpers, the derive macro and the
//! traits that make method syntax work.

pub use crate::types::{
    Column, ColumnWitness, NonNull, Nullability, Nullable, SqlEnum, SqlType, TypeWitness,
    UnknownLabel,
};
pub use besu_core::{TypeDef, Value};

#[cfg(feature = "derive")]
pub use besu_macros::{SqlEnum, Table};

pub use crate::batch::{Batch, BatchWrite};
pub use crate::database::{Database, RawQuery, Transaction, dry};
pub use crate::expr::{
    CaseBuilder, DateField, Expr, FrameBound, FrameUnits, Interval, IntoCondition, IntoExpr,
    IntoOrder, IntoSqlValue, IntoSubquery, NullsOrder, Order, OrderClause, Window, abs, and,
    array_agg, array_append, array_cat, array_contained_by, array_contains, array_get,
    array_length, array_overlaps, array_position, array_prepend, array_remove, array_replace,
    array_to_string, avg, between, bool_and, bool_or, btrim, cardinality, case, ceil, char_length,
    coalesce, coalesce3, coalesce4, coalesce5, concat, concat_ws, concat_ws4, concat3, concat4,
    concat5, concat6, concat7, concat8, contains_json, count, count_all, count_distinct,
    current_date, date_add, date_format, date_sub, date_trunc, datediff, dense_rank, eq, every,
    excluded, exists, exp, extract, floor, from_base64, from_unixtime, greatest, gt, gte, ilike,
    in_list, initcap, is_distinct_from, is_false, is_not_distinct_from, is_not_null, is_null,
    is_true, json_agg, json_array_agg, json_at, json_at_text, json_contained_by, json_get,
    json_has_all, json_has_any, json_has_key, json_key, json_key_text, json_matches,
    json_matches_path, json_merge_patch, json_object, json_object_agg, json_overlaps, json_query,
    json_remove, json_set, json_strip, json_text, json_value, jsonb_agg, lag, lag_by, lead,
    lead_by, least, left, length, like, ln, log, log10, lower, lpad, lt, lte, ltrim, matches, max,
    md5, min, ne, new_row, not, not_between, not_exists, not_ilike, not_in, not_like,
    not_regex_match, not_regex_match_ci, now, nullif, octet_length, or, power, rank, raw,
    regex_match, regex_match_ci, regexp_replace, regexp_substr, repeat, replace, reverse, right,
    round, round_to, row_number, rpad, rtrim, scalar, sha256, sign, split_part, sqrt, stddev,
    stddev_pop, string_agg, string_to_array, strpos, substr, substring, sum, to_base64, translate,
    trim, trunc, unix_timestamp, upper, var_pop, variance,
};
pub use crate::macros::Fragment;
pub use crate::query::{
    Alias, Cte, CteColumns, Delete, Derived, DynamicSelect, Insert, IntoQueryBody, JoinKind,
    OnConflict, Select, SetOp, SetQuery, Update, alias, alias_as, derived,
};
pub use crate::relation::{HasMany, WithRelation, has_many};
pub use crate::seed::{TableSeed, seed};
pub use crate::selection::Selection;
pub use crate::sql;
pub use crate::table::{Insertable, Set, Table};
