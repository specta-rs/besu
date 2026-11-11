/// The canonical, dialect-independent description of a SQL column type.
///
/// This is the single source of truth used by the query builder (for documentation and
/// casts), by schema generation (for DDL) and by migrations (for diffing). It is
/// deliberately *not* a database-native type: each [`Dialect`](crate::Dialect) maps a
/// [`TypeDef`] onto its own native spelling.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum TypeDef {
    /// `BOOLEAN`
    Bool,
    /// 16-bit signed integer.
    Int2,
    /// 32-bit signed integer.
    Int4,
    /// 64-bit signed integer.
    Int8,
    /// An unsigned integer of the given bit width. Databases have no portable unsigned
    /// integer, so dialects widen these to the smallest type that fits (`u64` becomes a
    /// `NUMERIC(20,0)` on PostgreSQL, for instance, and `u128` a `NUMERIC(39,0)`).
    UInt(u8),
    /// 32-bit float.
    Float4,
    /// 64-bit float.
    Float8,
    /// Arbitrary length text.
    Text,
    /// Fixed length text, as in `CHAR(n)`.
    Char(u32),
    /// Variable length text with a maximum, as in `VARCHAR(n)`.
    VarChar(u32),
    /// Arbitrary length binary data.
    Bytes,
    /// A UUID.
    Uuid,
    /// JSON text.
    Json,
    /// Binary JSON.
    Jsonb,
    /// A date with no time.
    Date,
    /// A time with no date.
    Time,
    /// A time with no date, carrying a timezone. This is PostgreSQL's `TIMETZ`.
    ///
    /// It is modelled so introspection round-trips and DDL renders it, but no driver
    /// decodes it: sqlx represents the type as its own `PgTimeTz` struct, which Besu's
    /// driver-independent [`Value`](crate::Value) has no variant for. Use `TimestampTz`
    /// (or `Time`) in code that reads and writes rows.
    TimeTz,
    /// A timestamp with no timezone.
    Timestamp,
    /// A timestamp with a timezone.
    TimestampTz,
    /// A fixed-precision decimal, `DECIMAL(precision, scale)`.
    Decimal(u8, u8),
    /// An arbitrary type the user asked for verbatim, for example a Postgres enum.
    ///
    /// Introspection uses this for database-native types Besu does not model, so it holds a
    /// `Cow` rather than a plain `&'static str`.
    Custom(alloc::borrow::Cow<'static, str>),
}
