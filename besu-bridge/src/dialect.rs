//! A dialect that delegates its spelling to another dialect and binds [`BridgeValue`]s.

use std::{borrow::Cow, marker::PhantomData};

use besu_core::dialect::{
    DistinctFromStyle, FullTextStyle, FunctionStyle, JsonPathStyle, RegexStyle,
};
use besu_core::{Dialect, Error, Placeholder, TypeDef, UpsertStyle, Value};

use crate::BridgeValue;

/// The dialect a [`Bridge`](crate::Bridge) speaks.
///
/// A bridge is a transport, not a database: the SQL is run by whatever server the requests go
/// to, and only that server knows which database it is. `BridgeDialect<D>` therefore takes
/// `D`'s placeholders, quoting, capabilities and type names, and substitutes the bridge's
/// wire value for the dialect's own bound value. A `Bridge<_, Postgres>` emits `$1` and
/// `DOUBLE PRECISION`; a `Bridge<_, MySql>` emits `?` and `` ` `` quoting; a
/// `Bridge<_, SQLite>` emits `?` and SQLite's type names.
///
/// `D`'s [`Dialect::bind`] is deliberately not called: an argument's value is encoded as a
/// [`BridgeValue`] instead, because the bridge never hands bytes to a local database. The
/// value travels losslessly, and the server binds it again with its own dialect.
///
/// There is one thing a wrapper cannot forward: [`Dialect::write_statement_suffix`]. A
/// PostgreSQL pool uses it to keep two statements with the same text but different parameter
/// types out of one prepared-statement cache entry. A bridge request is prepared on its own,
/// so there is no cache to protect and the suffix is omitted.
///
/// ```ignore
/// use besu_bridge::{Bridge, BridgeDialect};
/// use besu_postgres::Postgres;
///
/// let sql = besu::dry::<BridgeDialect<Postgres>>()
///     .select(users)
///     .filter(users.age.gt(18))
///     .to_sql()?;
/// assert!(sql.sql().contains("$1"));
/// ```
pub struct BridgeDialect<D: Dialect> {
    _dialect: PhantomData<fn() -> D>,
}

// The derives a dialect would normally have are written by hand so that `BridgeDialect<D>`
// is `Clone`, `Copy` and `Debug` whether or not `D` is: it holds no `D`, only a function
// pointer that names it.
impl<D: Dialect> Clone for BridgeDialect<D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D: Dialect> Copy for BridgeDialect<D> {}

impl<D: Dialect> std::fmt::Debug for BridgeDialect<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("BridgeDialect").field(&D::NAME).finish()
    }
}

impl<D: Dialect> PartialEq for BridgeDialect<D> {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl<D: Dialect> Eq for BridgeDialect<D> {}

impl<D: Dialect> Default for BridgeDialect<D> {
    fn default() -> Self {
        Self {
            _dialect: PhantomData,
        }
    }
}

impl<D: Dialect> Dialect for BridgeDialect<D> {
    /// The bridge's own wire value, not `D::Value`.
    type Value = BridgeValue;

    const NAME: &'static str = D::NAME;
    const PLACEHOLDER: Placeholder = D::PLACEHOLDER;
    const SUPPORTS_RETURNING: bool = D::SUPPORTS_RETURNING;
    const SUPPORTS_ILIKE: bool = D::SUPPORTS_ILIKE;
    const SUPPORTS_ARRAYS: bool = D::SUPPORTS_ARRAYS;
    const SUPPORTS_FULL_JOIN: bool = D::SUPPORTS_FULL_JOIN;
    const SUPPORTS_ALTER_COLUMN_TYPE: bool = D::SUPPORTS_ALTER_COLUMN_TYPE;
    const REQUIRES_LIMIT_WITH_OFFSET: bool = D::REQUIRES_LIMIT_WITH_OFFSET;
    const SUPPORTS_NULLS_ORDER: bool = D::SUPPORTS_NULLS_ORDER;
    const SUPPORTS_DEFAULT_IN_VALUES: bool = D::SUPPORTS_DEFAULT_IN_VALUES;
    const UNBOUNDED_LIMIT: i64 = D::UNBOUNDED_LIMIT;
    const UPSERT: UpsertStyle = D::UPSERT;
    const SUPPORTS_TARGETLESS_DO_UPDATE: bool = D::SUPPORTS_TARGETLESS_DO_UPDATE;
    const JSON_PATH: JsonPathStyle = D::JSON_PATH;
    const FULL_TEXT: FullTextStyle = D::FULL_TEXT;
    const SUPPORTS_DELETE_LIMIT: bool = D::SUPPORTS_DELETE_LIMIT;
    const MAX_PARAMS: usize = D::MAX_PARAMS;
    const SUPPORTS_RIGHT_JOIN: bool = D::SUPPORTS_RIGHT_JOIN;
    const SUPPORTS_ROW_LOCKING: bool = D::SUPPORTS_ROW_LOCKING;
    const ROUND_TO_NEEDS_NUMERIC: bool = D::ROUND_TO_NEEDS_NUMERIC;
    const SUPPORTS_INTERSECT_ALL: bool = D::SUPPORTS_INTERSECT_ALL;
    const SUPPORTS_EXCEPT_ALL: bool = D::SUPPORTS_EXCEPT_ALL;
    const DISTINCT_FROM: DistinctFromStyle = D::DISTINCT_FROM;
    const REGEX: RegexStyle = D::REGEX;

    fn function(name: &str) -> Option<FunctionStyle> {
        D::function(name)
    }

    fn bind(value: Value) -> Result<Self::Value, Error> {
        BridgeValue::from_value(value)
    }

    fn write_placeholder(out: &mut String, index: usize) {
        D::write_placeholder(out, index);
    }

    fn quote(ident: &str) -> String {
        D::quote(ident)
    }

    fn write_ident(out: &mut String, ident: &str) {
        D::write_ident(out, ident);
    }

    fn type_name(ty: TypeDef) -> Cow<'static, str> {
        D::type_name(ty)
    }

    fn cast_type_name(ty: TypeDef) -> Cow<'static, str> {
        D::cast_type_name(ty)
    }

    /// Writes nothing. See the type documentation for why a bridge has no statement suffix.
    fn write_statement_suffix(_out: &mut String, _args: &[Self::Value]) {}
}
