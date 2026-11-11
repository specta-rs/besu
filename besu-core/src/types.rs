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
    /// `NUMERIC(20,0)` on PostgreSQL, for instance).
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
