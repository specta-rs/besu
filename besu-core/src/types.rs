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
    /// A time with an explicit fractional-second precision, as in PostgreSQL's
    /// `TIME(3)` or MySQL's `TIME(6)`.
    ///
    /// This is a separate variant from [`TypeDef::Time`] rather than a field on it so that
    /// the precision is visible only when the database actually has one. `TIME` with no
    /// modifier is not `TIME(6)`: the two are distinct PostgreSQL types, and folding them
    /// would make `pull` → `push` silently change a column's precision.
    TimeP(u8),
    /// A time with no date, carrying a timezone. This is PostgreSQL's `TIMETZ`.
    ///
    /// It is modelled so introspection round-trips and DDL renders it, but no driver
    /// decodes it: sqlx represents the type as its own `PgTimeTz` struct, which Besu's
    /// driver-independent [`Value`](crate::Value) has no variant for. Use `TimestampTz`
    /// (or `Time`) in code that reads and writes rows.
    TimeTz,
    /// A time with a timezone and an explicit fractional-second precision, as in
    /// PostgreSQL's `TIMETZ(3)`. See [`TypeDef::TimeP`].
    TimeTzP(u8),
    /// A timestamp with no timezone.
    Timestamp,
    /// A timestamp with an explicit fractional-second precision, as in
    /// `TIMESTAMP(3)`. See [`TypeDef::TimeP`].
    TimestampP(u8),
    /// A timestamp with a timezone.
    TimestampTz,
    /// A timestamp with a timezone and an explicit precision, as in
    /// `TIMESTAMPTZ(3)`. See [`TypeDef::TimeP`].
    TimestampTzP(u8),
    /// A fixed-precision decimal, `DECIMAL(precision, scale)`.
    Decimal(u8, u8),
    /// A fixed-length bit string, as in PostgreSQL's `BIT(n)`.
    ///
    /// It is modelled so a `pull` → `push` keeps the width, but no driver decodes it: there
    /// is no bit-string [`Value`](crate::Value) variant.
    Bit(u32),
    /// A variable-length bit string, as in PostgreSQL's `BIT VARYING(n)`.
    ///
    /// A width of zero means the declaration had no modifier, which is unlimited. See
    /// [`TypeDef::Bit`] for why it is modelled but not decoded.
    VarBit(u32),
    /// A type carrying an explicit collation, as in `TEXT COLLATE "C"`.
    ///
    /// Collation is part of a column's type in every dialect Besu supports: it changes
    /// comparisons and ordering, so a column read back without it would not describe the
    /// column that exists. The inner type is what the column stores; the `Cow` is the
    /// collation's name exactly as the database spells it.
    Collated(Box<TypeDef>, alloc::borrow::Cow<'static, str>),
    /// An array of another type, as in PostgreSQL's `<inner>[]`.
    ///
    /// Every supported database has a different answer here: PostgreSQL has real arrays,
    /// MySQL and SQLite have none. The variant is still dialect-independent, because a
    /// snapshot taken from PostgreSQL has to describe what is there; the other dialects
    /// render and bind it as unsupported rather than pretending otherwise.
    ///
    /// The element is a [`TypeRef`] rather than a `Box<TypeDef>` because a declared column's
    /// type is part of a `const` [`TableDef`](crate::TableDef), and a boxed value cannot be
    /// constructed in a constant expression.
    Array(TypeRef),
    /// An arbitrary type the user asked for verbatim, for example a Postgres enum.
    ///
    /// Introspection uses this for database-native types Besu does not model, so it holds a
    /// `Cow` rather than a plain `&'static str`.
    Custom(alloc::borrow::Cow<'static, str>),
}

/// A reference to a [`TypeDef`] that can be written in a `const` expression.
///
/// A declared column's type lives in a `const` [`TableDef`](crate::TableDef), and a value
/// whose element is another type — an array — cannot hold a `Box` there: allocating is not
/// allowed in a constant expression. The two shapes this type has are exactly the two a type
/// can arrive in: a literal written in the source (borrowed, which is the `const` case) and a
/// type computed at run time by introspection or deserialised from a snapshot (owned).
///
/// Comparison, hashing and serialisation all see through the holder, so an array read back
/// from a snapshot equals the one a struct declared.
#[derive(Debug, Clone)]
pub enum TypeRef {
    /// A type written in a constant expression.
    Static(&'static TypeDef),
    /// A type computed at run time.
    Owned(alloc::boxed::Box<TypeDef>),
}

// Derived comparison would compare the holder, not the type: `Static(&Int4)` and
// `Owned(Int4)` describe the same column, and the doc above promises they compare equal.
// A declared array and the one introspection reports must be equal, or every `push` sees a
// type change.
impl PartialEq for TypeRef {
    fn eq(&self, other: &Self) -> bool {
        self.as_type() == other.as_type()
    }
}

impl Eq for TypeRef {}

impl core::hash::Hash for TypeRef {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.as_type().hash(state);
    }
}

impl TypeRef {
    /// Borrow a type that is already static, as a `const` declaration does.
    pub const fn from_static(ty: &'static TypeDef) -> Self {
        Self::Static(ty)
    }

    /// Take ownership of a type computed at run time.
    pub fn owned(ty: TypeDef) -> Self {
        Self::Owned(alloc::boxed::Box::new(ty))
    }

    /// The type itself.
    pub fn as_type(&self) -> &TypeDef {
        match self {
            Self::Static(ty) => ty,
            Self::Owned(ty) => ty,
        }
    }
}

impl core::ops::Deref for TypeRef {
    type Target = TypeDef;

    fn deref(&self) -> &Self::Target {
        self.as_type()
    }
}

impl From<TypeDef> for TypeRef {
    fn from(ty: TypeDef) -> Self {
        Self::owned(ty)
    }
}

impl serde::Serialize for TypeRef {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.as_type().serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for TypeRef {
    /// A deserialised type is always owned: a snapshot borrows nothing from the input, so
    /// there is no static allocation to point at.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::owned(TypeDef::deserialize(deserializer)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_type_reference_compares_by_its_type() {
        // A declared array holds a `Static` reference; the same array read back from a
        // snapshot holds an `Owned` one. If those compared unequal, every `push` would see a
        // type change on an array column and `pull` → `push` would never converge.
        const INT4: TypeDef = TypeDef::Int4;
        let borrowed = TypeRef::from_static(&INT4);
        let owned = TypeRef::owned(TypeDef::Int4);
        assert_eq!(borrowed, owned);
        assert_eq!(
            TypeDef::Array(borrowed.clone()),
            TypeDef::Array(owned.clone())
        );

        use core::hash::{Hash as _, Hasher as _};
        use std::hash::DefaultHasher;
        let mut a = DefaultHasher::new();
        borrowed.hash(&mut a);
        let mut b = std::hash::DefaultHasher::new();
        owned.hash(&mut b);
        assert_eq!(a.finish(), b.finish());
    }
}
