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
    /// MySQL's `BIT(n)` and PostgreSQL's type share the same canonical form; SQLite stores
    /// the bits as text. The value is [`BitString`](crate::BitString).
    Bit(u32),
    /// A variable-length bit string, as in PostgreSQL's `BIT VARYING(n)`.
    ///
    /// A width of zero means the declaration had no modifier, which is unlimited. See
    /// [`TypeDef::Bit`].
    VarBit(u32),
    /// A SQL interval.
    ///
    /// PostgreSQL has a native `INTERVAL`; MySQL has none, and SQLite stores the canonical
    /// text. The value is [`SqlInterval`](crate::SqlInterval).
    Interval,
    /// A PostgreSQL `INET` address.
    ///
    /// The value is [`Inet`](crate::Inet). MySQL and SQLite store the canonical text.
    Inet,
    /// A PostgreSQL `CIDR` network.
    ///
    /// The value is [`Cidr`](crate::Cidr). MySQL and SQLite store the canonical text.
    Cidr,
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
    /// A PostgreSQL range type, such as `int4range` or `tstzrange`.
    ///
    /// The element is the range's subtype, which decides the value's wire format and the
    /// name PostgreSQL gives the type: `Int4` is `int4range`, `Timestamp` is `tsrange`, and
    /// so on. Only the six built-in range types are modelled; a user-defined range over
    /// another type stays a [`TypeDef::Custom`].
    ///
    /// A [`TypeRef`] rather than a `Box<TypeDef>`, for the same reason
    /// [`TypeDef::Array`] uses one: a declared column's type is a constant.
    ///
    /// PostgreSQL has ranges; MySQL and SQLite do not, and store the value's canonical text
    /// as `TEXT`.
    Range(TypeRef),
    /// A PostGIS `GEOMETRY` column, optionally narrowed to a shape and an SRID.
    ///
    /// PostGIS reports the narrowing through `format_type` as `geometry(Point,4326)`, so the
    /// shape and SRID are part of the column's type. A plain `GEOMETRY` column is
    /// `kind: GeometryKind::Geometry, srid: None`.
    Geometry {
        /// The shape the column accepts.
        kind: crate::scalars::GeometryKind,
        /// The spatial reference system identifier, when the column names one.
        srid: Option<u32>,
    },
    /// A PostGIS `GEOGRAPHY` column, the ellipsoidal counterpart of
    /// [`TypeDef::Geometry`].
    Geography {
        /// The shape the column accepts.
        kind: crate::scalars::GeometryKind,
        /// The spatial reference system identifier, when the column names one.
        srid: Option<u32>,
    },
    /// A `pgvector` vector column, `VECTOR(n)`.
    ///
    /// The dimension is part of the declared type, exactly as `VARCHAR(n)`'s length is. A
    /// dimension of zero means the declaration had no modifier, which `pgvector` accepts as
    /// "any dimension"; introspection reports it the same way, so a push converges. The
    /// value is [`Vector`](crate::Vector), sent untyped in the extension's binary form.
    ///
    /// SQLite and MySQL store the `[1,2,3]` text in a `TEXT` column.
    Vector(u32),
    /// An enum whose labels are part of the declaration.
    ///
    /// MySQL spells one inline in the column — `ENUM('a','b')` *is* the type — while
    /// PostgreSQL has a named type that lives beside the table, and SQLite has no enum at
    /// all. The variants carry both halves: a [`name`](EnumDef::name), which PostgreSQL
    /// creates and names, and the [`labels`](EnumDef::labels), which every dialect needs to
    /// spell or enforce the column.
    ///
    /// The name is empty when there is nothing to name: a MySQL column introspected back
    /// reports its labels with no name, and a declaration whose enum has no name is stored
    /// as `TEXT` on PostgreSQL. Each dialect folds the type in
    /// [`normalise_type`](crate::Dialect) so a declaration and the column the database
    /// reports back compare equal.
    Enum(EnumDef),
    /// A MySQL `SET`: a column that holds any combination of a fixed label set.
    ///
    /// The labels are the [`EnumLabels`] holder an enum uses. MySQL is the only dialect with
    /// the type, so the DDL is inline — `SET('a','b')` — exactly as an inline enum is. The
    /// value is a text string of the chosen labels separated by commas, which is the form
    /// MySQL itself sends; [`TypeDef::Set`] carries the labels so a declaration and the
    /// database converge.
    Set(EnumLabels),
    /// A PostgreSQL domain: a name for a base type, with optional constraints.
    ///
    /// PostgreSQL reports a domain as a type of its own. Keeping only [`TypeDef::Custom`]
    /// would make the column impossible to describe, so the name and the base type are both
    /// modelled: DDL creates the domain, the column is declared with the domain's name, and
    /// a value is decoded and written through the base type's binary format. The constraints
    /// and default a domain can carry are not modelled; see the PostgreSQL column-type page.
    Domain(DomainDef),
    /// An arbitrary type the user asked for verbatim, for example a Postgres domain.
    ///
    /// Introspection uses this for database-native types Besu does not model, so it holds a
    /// `Cow` rather than a plain `&'static str`.
    Custom(alloc::borrow::Cow<'static, str>),
}

/// A PostgreSQL domain: a name and the type it is built on.
///
/// The name is qualified the same way a table or an enum is: `public.positive` in a schema
/// that is not the connection's default, and `positive` otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct DomainDef {
    /// The domain's name, possibly schema-qualified.
    pub name: alloc::borrow::Cow<'static, str>,
    /// The type the domain is built on.
    ///
    /// A [`TypeRef`] rather than a `Box<TypeDef>` because a declared type is part of a `const`
    /// [`TableDef`](crate::TableDef), and a boxed value cannot be built in a constant.
    pub base: TypeRef,
}

impl DomainDef {
    /// Construct a domain in a constant expression.
    pub const fn new(name: &'static str, base: &'static TypeDef) -> Self {
        Self {
            name: alloc::borrow::Cow::Borrowed(name),
            base: TypeRef::from_static(base),
        }
    }

    /// Construct a domain whose name and base type were computed at run time.
    pub fn owned(name: impl Into<alloc::string::String>, base: TypeDef) -> Self {
        Self {
            name: alloc::borrow::Cow::Owned(name.into()),
            base: TypeRef::owned(base),
        }
    }
}

/// A set of enum labels, in the order they were declared.
///
/// The two shapes are the two a label set arrives in: a literal written in the source, which
/// has to be constructible in the `const` metadata a [`TableDef`](crate::TableDef) holds, and
/// one read back from a database or deserialised from a snapshot. Comparison, hashing and
/// serialisation see through the holder, so a declared enum equals the one introspection
/// reports.
#[derive(Debug, Clone)]
pub enum EnumLabels {
    /// Labels written in a constant expression.
    Static(&'static [&'static str]),
    /// Labels computed at run time, by introspection or deserialisation.
    Owned(alloc::vec::Vec<alloc::string::String>),
}

// Derived comparison would compare the holder, not the labels: `Static(&["a"])` and
// `Owned(vec!["a"])` describe the same enum, and a declaration must equal what the database
// reports back or every `push` would see a type change.
impl PartialEq for EnumLabels {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .zip(other.iter())
                .all(|(left, right)| left == right)
    }
}

impl Eq for EnumLabels {}

impl core::hash::Hash for EnumLabels {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.len().hash(state);
        for label in self.iter() {
            label.hash(state);
        }
    }
}

impl EnumLabels {
    /// Borrow a label set that is already static, as a `const` declaration does.
    pub const fn from_static(labels: &'static [&'static str]) -> Self {
        Self::Static(labels)
    }

    /// Take ownership of a label set computed at run time.
    pub fn owned(labels: alloc::vec::Vec<alloc::string::String>) -> Self {
        Self::Owned(labels)
    }

    /// How many labels there are.
    pub fn len(&self) -> usize {
        match self {
            Self::Static(labels) => labels.len(),
            Self::Owned(labels) => labels.len(),
        }
    }

    /// Whether there are no labels.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The label at `index`, if there is one.
    pub fn get(&self, index: usize) -> Option<&str> {
        match self {
            Self::Static(labels) => labels.get(index).copied(),
            Self::Owned(labels) => labels.get(index).map(alloc::string::String::as_str),
        }
    }

    /// The labels, in declaration order.
    pub fn iter(&self) -> EnumLabelsIter<'_> {
        EnumLabelsIter {
            labels: self,
            index: 0,
        }
    }

    /// The labels as a slice of string slices.
    ///
    /// A static set is borrowed; an owned one cannot hand out a `&[&str]` because it stores
    /// its labels as `String`s, so this allocates a vector of borrows. It exists as the
    /// slice-shaped accessor; [`EnumLabels::iter`] is the allocation-free one.
    pub fn as_slice(&self) -> alloc::borrow::Cow<'_, [&str]> {
        match self {
            Self::Static(labels) => alloc::borrow::Cow::Borrowed(labels),
            Self::Owned(labels) => alloc::borrow::Cow::Owned(
                labels
                    .iter()
                    .map(alloc::string::String::as_str)
                    .collect::<alloc::vec::Vec<_>>(),
            ),
        }
    }

    /// A copy of every label as an owned string.
    pub fn to_vec(&self) -> alloc::vec::Vec<alloc::string::String> {
        self.iter()
            .map(alloc::string::ToString::to_string)
            .collect()
    }
}

impl<'a> IntoIterator for &'a EnumLabels {
    type Item = &'a str;
    type IntoIter = EnumLabelsIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// An iterator over an [`EnumLabels`].
#[derive(Debug, Clone)]
pub struct EnumLabelsIter<'a> {
    labels: &'a EnumLabels,
    index: usize,
}

impl<'a> Iterator for EnumLabelsIter<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        let label = self.labels.get(self.index)?;
        self.index += 1;
        Some(label)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.labels.len().saturating_sub(self.index);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for EnumLabelsIter<'_> {}

impl serde::Serialize for EnumLabels {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq as _;

        let mut sequence = serializer.serialize_seq(Some(self.len()))?;
        for label in self.iter() {
            sequence.serialize_element(label)?;
        }
        sequence.end()
    }
}

impl<'de> serde::Deserialize<'de> for EnumLabels {
    /// A deserialised label set is always owned: a snapshot borrows nothing from the input,
    /// so there is no static slice to point at.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        alloc::vec::Vec::<alloc::string::String>::deserialize(deserializer).map(Self::Owned)
    }
}

/// An enum type: a name, and the labels it allows.
///
/// The name is the PostgreSQL type name, and empty for an inline enum that has none. The
/// labels are the values a column of this type accepts, in the order they were declared,
/// which is also the order PostgreSQL compares them in.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct EnumDef {
    /// The type's name, or an empty string when the enum is spelled inline.
    pub name: alloc::borrow::Cow<'static, str>,
    /// The allowed labels, in order.
    pub labels: EnumLabels,
}

impl EnumDef {
    /// Construct an enum in a constant expression, as `#[derive(SqlEnum)]` metadata is.
    pub const fn new(name: &'static str, labels: &'static [&'static str]) -> Self {
        Self {
            name: alloc::borrow::Cow::Borrowed(name),
            labels: EnumLabels::Static(labels),
        }
    }

    /// Construct an enum whose name and labels were computed at run time.
    pub fn owned(
        name: impl Into<alloc::string::String>,
        labels: impl IntoIterator<Item = impl Into<alloc::string::String>>,
    ) -> Self {
        Self {
            name: alloc::borrow::Cow::Owned(name.into()),
            labels: EnumLabels::Owned(labels.into_iter().map(Into::into).collect()),
        }
    }

    /// How many labels the type allows.
    pub fn len(&self) -> usize {
        self.labels.len()
    }

    /// Whether the type allows no labels at all.
    pub fn is_empty(&self) -> bool {
        self.labels.is_empty()
    }

    /// The labels, in declaration order.
    pub fn iter(&self) -> EnumLabelsIter<'_> {
        self.labels.iter()
    }
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

    #[test]
    fn an_enum_is_constructible_in_a_constant() {
        // `#[derive(Table)]` puts a column's type in a `const TableDef`, so an enum has to
        // be constructible without allocating.
        const MOOD: TypeDef = TypeDef::Enum(EnumDef::new("mood", &["happy", "meh", "sad"]));
        let TypeDef::Enum(def) = &MOOD else {
            unreachable!("the constant must be an enum");
        };
        assert_eq!(def.name, "mood");
        assert_eq!(def.iter().collect::<Vec<_>>(), ["happy", "meh", "sad"]);
        assert_eq!(def.labels.as_slice().as_ref(), ["happy", "meh", "sad"]);
        assert_eq!(def.len(), 3);
    }

    #[test]
    fn static_and_owned_enum_labels_are_the_same_enum() {
        // A declared enum holds static labels; introspection produces owned ones. If the two
        // were not equal, every `push` would see the column's type change.
        const DECLARED: EnumDef = EnumDef::new("mood", &["happy", "sad"]);
        let introspected = EnumDef::owned("mood", ["happy", "sad"]);
        assert_eq!(DECLARED, introspected);
        assert_eq!(DECLARED.labels, introspected.labels);

        use core::hash::{Hash as _, Hasher as _};
        let mut declared = std::hash::DefaultHasher::new();
        DECLARED.hash(&mut declared);
        let mut introspected_hash = std::hash::DefaultHasher::new();
        introspected.hash(&mut introspected_hash);
        assert_eq!(declared.finish(), introspected_hash.finish());
    }

    #[test]
    fn an_enum_serialises_as_its_name_and_labels() {
        const DECLARED: TypeDef = TypeDef::Enum(EnumDef::new("mood", &["happy", "sad"]));
        let json = serde_json::to_string(&DECLARED).expect("an enum must serialise");
        assert_eq!(json, r#"{"Enum":{"name":"mood","labels":["happy","sad"]}}"#);

        // A static label set and an owned one write the same JSON, and both read back owned.
        let owned = serde_json::to_value(TypeDef::Enum(EnumDef::owned("mood", ["happy", "sad"])))
            .expect("an owned enum must serialise");
        assert_eq!(
            serde_json::to_value(&DECLARED).expect("a declared enum must serialise"),
            owned
        );
        let decoded: TypeDef =
            serde_json::from_str(r#"{"Enum":{"name":"mood","labels":["happy","sad"]}}"#)
                .expect("an enum must deserialise");
        assert_eq!(decoded, DECLARED);
        assert!(matches!(
            decoded,
            TypeDef::Enum(EnumDef {
                labels: EnumLabels::Owned(_),
                ..
            })
        ));
    }

    #[test]
    fn an_enum_with_no_labels_is_empty() {
        let def = EnumDef::owned("empty", Vec::<String>::new());
        assert!(def.is_empty());
        assert_eq!(def.iter().next(), None);
    }

    #[test]
    fn a_domain_is_constructible_in_a_constant() {
        // A domain's name and base type are part of a declared column's metadata, which lives
        // in a `const TableDef`, so neither may allocate.
        const POSITIVE: TypeDef = TypeDef::Domain(DomainDef::new("positive", &TypeDef::Int4));
        let TypeDef::Domain(def) = &POSITIVE else {
            unreachable!("the constant must be a domain");
        };
        assert_eq!(def.name, "positive");
        assert_eq!(def.base.as_type(), &TypeDef::Int4);

        let owned = DomainDef::owned("positive", TypeDef::Int4);
        assert_eq!(*def, owned);
    }

    #[test]
    fn a_domain_serialises_its_name_and_base() {
        let ty = TypeDef::Domain(DomainDef::new("positive", &TypeDef::Int4));
        let json = serde_json::to_string(&ty).expect("a domain must serialise");
        assert_eq!(json, r#"{"Domain":{"name":"positive","base":"Int4"}}"#);
        let decoded: TypeDef = serde_json::from_str(&json).expect("a domain must deserialise");
        assert_eq!(decoded, ty);
    }

    #[test]
    fn a_set_compares_by_its_labels() {
        // A declared `SET` holds static labels while the one introspection reports is owned;
        // they are the same type, so every push does not see a change.
        let declared = TypeDef::Set(EnumLabels::from_static(&["read", "write"]));
        let introspected = TypeDef::Set(EnumLabels::owned(vec!["read".into(), "write".into()]));
        assert_eq!(declared, introspected);
    }

    #[test]
    fn a_range_geometry_and_vector_are_constructible_in_a_constant() {
        // A declared column's type is part of a `const TableDef`, so every new variant has to
        // be buildable without allocating.
        const RANGE: TypeDef = TypeDef::Range(TypeRef::from_static(&TypeDef::Int4));
        const POINT: TypeDef = TypeDef::Geometry {
            kind: crate::scalars::GeometryKind::Point,
            srid: Some(4326),
        };
        const GEOGRAPHY: TypeDef = TypeDef::Geography {
            kind: crate::scalars::GeometryKind::Geometry,
            srid: None,
        };
        const VECTOR: TypeDef = TypeDef::Vector(1536);

        assert_eq!(
            RANGE,
            TypeDef::Range(TypeRef::owned(TypeDef::Int4)),
            "a static and an owned element must be the same range"
        );
        assert_eq!(
            POINT,
            TypeDef::Geometry {
                kind: crate::scalars::GeometryKind::from_name("Point"),
                srid: Some(4326),
            }
        );
        assert_eq!(
            GEOGRAPHY,
            TypeDef::Geography {
                kind: crate::scalars::GeometryKind::Geometry,
                srid: None
            }
        );
        assert_eq!(VECTOR, TypeDef::Vector(1536));
    }

    #[test]
    fn the_new_types_serialise_and_deserialise() {
        let types = [
            TypeDef::Range(TypeRef::from_static(&TypeDef::TimestampTz)),
            TypeDef::Geometry {
                kind: crate::scalars::GeometryKind::MultiPolygon,
                srid: Some(3857),
            },
            TypeDef::Geography {
                kind: crate::scalars::GeometryKind::Other("PolyhedralSurface".into()),
                srid: None,
            },
            TypeDef::Vector(3),
        ];
        for ty in types {
            let json = serde_json::to_string(&ty).expect("the type must serialise");
            let decoded: TypeDef = serde_json::from_str(&json).expect("the type must deserialise");
            assert_eq!(decoded, ty, "{json}");
        }
    }
}
