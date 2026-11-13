use besu_core::{
    BitString, Cidr, Decode, Driver, Error, Geometry, Inet, Range, Sql, SqlInterval,
    TransactionDriver, Value, Vector,
};
use chrono::Timelike;
use sqlx::{
    Arguments, Column as _, PgPool, Postgres as SqlxPostgres, Row, ValueRef,
    postgres::{
        PgArgumentBuffer, PgArguments, PgQueryResult, PgRow, PgTypeInfo, PgTypeKind, PgValueFormat,
        PgValueRef,
    },
};

use crate::Postgres;

/// The OID of PostgreSQL's built-in `NUMERIC` type, which is stable across servers.
const NUMERIC_OID: u32 = 1700;

/// The OID of PostgreSQL's built-in `REAL` (`FLOAT4`) type.
const FLOAT4_OID: u32 = 700;

/// The OID of PostgreSQL's built-in `INTERVAL` type.
const INTERVAL_OID: u32 = 1186;

/// The OID of PostgreSQL's built-in `INET` type.
const INET_OID: u32 = 869;

/// The OID of PostgreSQL's built-in `CIDR` type.
const CIDR_OID: u32 = 650;

/// The OID of PostgreSQL's built-in `BIT` type.
const BIT_OID: u32 = 1560;

/// The OID of PostgreSQL's built-in `VARBIT` type.
const VARBIT_OID: u32 = 1562;

/// The type behind a domain, or the type itself when it is not one.
///
/// sqlx resolves a domain to its base type in the statement's type description, so a value
/// read from a domain column knows what it really is. Every decoder that branches on an OID
/// goes through this, which is what makes a domain over `NUMERIC` read like a `NUMERIC`.
fn base_type(ty: &PgTypeInfo) -> &PgTypeInfo {
    let mut current = ty;
    while let PgTypeKind::Domain(base) = current.kind() {
        current = base;
    }
    current
}

/// Whether a value's declared SQL type is the built-in type with this OID, reaching through
/// a domain.
fn type_is(value: &PgValueRef<'_>, oid: u32) -> bool {
    let ty = value.type_info();
    base_type(ty.as_ref()).type_eq(&PgTypeInfo::with_oid(sqlx::postgres::types::Oid(oid)))
}

/// Whether a value's declared type is a domain.
fn is_domain(value: &PgValueRef<'_>) -> bool {
    matches!(value.type_info().kind(), PgTypeKind::Domain(_))
}

/// A [`besu_core::Driver`] for PostgreSQL, backed by [`sqlx`].
///
/// The pool is created with [`sqlx`]'s defaults: at most ten connections, opened lazily as
/// queries need them. Wrap an existing pool with [`Sqlx::from_pool`] when the application
/// needs to configure it differently.
#[derive(Debug, Clone)]
pub struct Sqlx {
    pool: PgPool,
}

impl Sqlx {
    /// Open a connection pool from a PostgreSQL connection string.
    ///
    /// This is an alias for [`Sqlx::connect`], for symmetry with the other Besu drivers.
    pub async fn new(url: impl AsRef<str>) -> Result<Self, sqlx::Error> {
        Self::connect(url.as_ref()).await
    }

    /// Open a connection pool from a connection string such as
    /// `postgres://user:password@localhost/database`.
    pub async fn connect(url: &str) -> Result<Self, sqlx::Error> {
        Ok(Self {
            pool: PgPool::connect(url).await?,
        })
    }

    /// Wrap an existing pool.
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The underlying pool, for anything Besu does not cover.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

/// A fixed-precision decimal, encoded directly into PostgreSQL's binary `NUMERIC` format.
///
/// sqlx implements `NUMERIC` only for `BigDecimal`, which allocates a digit vector and then
/// walks it again to write the wire format. A decimal column's value is already exact text,
/// so parsing those digits straight into the wire format keeps the encode path allocation-free
/// and leaves the parameter's SQL type exactly `NUMERIC`.
#[derive(Debug, Clone, Copy)]
struct NumericText<'a> {
    text: &'a str,
}

impl sqlx::Type<SqlxPostgres> for NumericText<'_> {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::with_oid(sqlx::postgres::types::Oid(NUMERIC_OID))
    }
}

impl sqlx::Encode<'_, SqlxPostgres> for NumericText<'_> {
    fn encode_by_ref(
        &self,
        buffer: &mut PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        let bytes = numeric_from_text(self.text)
            .map_err(|reason| <Sqlx as Driver>::error(Error::encode("decimal", reason)))?;
        buffer.extend_from_slice(&bytes);
        Ok(sqlx::encode::IsNull::No)
    }

    fn size_hint(&self) -> usize {
        // The header plus at most one two-byte digit per four decimal characters.
        8 + self.text.len().div_ceil(4) * 2
    }
}

/// Encode decimal text into PostgreSQL's binary `NUMERIC` representation.
///
/// The wire format is a header of `ndigits`, `weight`, `sign` and `dscale`, followed by
/// base-10,000 digits, most significant first. `weight` is the exponent of the first digit,
/// and `dscale` is how many digits after the decimal point to display.
fn numeric_from_text(text: &str) -> Result<Vec<u8>, &'static str> {
    let text = text.trim();
    let (negative, digits) = match text.strip_prefix('-') {
        Some(digits) => (true, digits),
        None => (false, text),
    };
    let (integer, fraction) = match digits.split_once('.') {
        Some((integer, fraction)) => (integer, fraction),
        None => (digits, ""),
    };
    if (integer.is_empty() && fraction.is_empty())
        || !integer.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("the decimal is not a plain digit string");
    }

    // Each group is four decimal digits, and the groups are aligned on the decimal point, so
    // the integer part is padded on the left and the fraction on the right.
    let integer_padding = (4 - integer.len() % 4) % 4;
    let mut groups: Vec<u16> = Vec::with_capacity(integer.len() / 4 + fraction.len() / 4 + 2);
    let mut group = 0u16;
    let mut digits_in_group = 0;
    for _ in 0..integer_padding {
        group *= 10;
        digits_in_group += 1;
    }
    for byte in integer.bytes().chain(fraction.bytes()) {
        group = group * 10 + u16::from(byte - b'0');
        digits_in_group += 1;
        if digits_in_group == 4 {
            groups.push(group);
            group = 0;
            digits_in_group = 0;
        }
    }
    if digits_in_group != 0 {
        // The fraction was padded on the right, so fill the last group out with zeros.
        for _ in digits_in_group..4 {
            group *= 10;
        }
        groups.push(group);
    }

    let integer_groups = integer.len().div_ceil(4);
    // `weight` counts from the most significant group, which is the last integer group.
    let mut weight = integer_groups as i32 - 1;

    let mut first = 0;
    while first < groups.len() && groups[first] == 0 {
        first += 1;
    }
    let mut end = groups.len();
    while end > first && groups[end - 1] == 0 {
        end -= 1;
    }

    let ndigits = (end - first) as i16;
    weight -= first as i32;
    let sign: u16 = if ndigits == 0 {
        // Zero has no digits and no weight, whatever sign the text carried.
        weight = 0;
        0
    } else if negative {
        NUMERIC_SIGN_NEGATIVE
    } else {
        0
    };
    let dscale = i16::try_from(fraction.len()).map_err(|_| "the decimal scale is too large")?;

    let mut out = Vec::with_capacity(8 + (end - first) * 2);
    out.extend_from_slice(&ndigits.to_be_bytes());
    out.extend_from_slice(&(weight as i16).to_be_bytes());
    out.extend_from_slice(&sign.to_be_bytes());
    out.extend_from_slice(&dscale.to_be_bytes());
    for group in &groups[first..end] {
        out.extend_from_slice(&group.to_be_bytes());
    }
    Ok(out)
}

/// A bound array, encoded into PostgreSQL's binary array format.
///
/// The parameter's declared type is OID 0, which tells the server to infer it from the
/// statement — exactly as [`UntypedNull`] does. The array payload itself still carries the
/// element OID, taken from the value's declared [`TypeDef`], and PostgreSQL's `array_recv`
/// insists that it matches the type it inferred. Inferring rather than declaring is what lets
/// one runtime encoder serve every element type without a type per generic instantiation.
#[derive(Debug, Clone, Copy)]
struct ArrayValue<'a> {
    element: &'a besu_core::TypeDef,
    values: &'a [Value],
}

impl sqlx::Type<SqlxPostgres> for ArrayValue<'_> {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::with_oid(sqlx::postgres::types::Oid(0))
    }
}

impl sqlx::Encode<'_, SqlxPostgres> for ArrayValue<'_> {
    fn encode_by_ref(
        &self,
        buffer: &mut PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        let element = element_oid(self.element).ok_or_else(|| {
            <Sqlx as Driver>::error(Error::encode(
                "array",
                "this element type has no PostgreSQL binary representation Besu can write",
            ))
        })?;

        let has_null = self.values.iter().any(Value::is_null);
        // `ndim`, `has_null` and the element's OID make up the header, then one
        // `(length, lower_bound)` pair per dimension. PostgreSQL's arrays are one-based.
        buffer.extend_from_slice(&1_i32.to_be_bytes());
        buffer.extend_from_slice(&i32::from(has_null).to_be_bytes());
        buffer.extend_from_slice(&element.to_be_bytes());
        buffer.extend_from_slice(
            &i32::try_from(self.values.len())
                .map_err(|_| <Sqlx as Driver>::error(Error::encode("array", "too many elements")))?
                .to_be_bytes(),
        );
        buffer.extend_from_slice(&1_i32.to_be_bytes());

        let mut payload = Vec::new();
        for value in self.values {
            if value.is_null() {
                buffer.extend_from_slice(&(-1_i32).to_be_bytes());
                continue;
            }
            payload.clear();
            encode_element(self.element, value, &mut payload)
                .map_err(|reason| <Sqlx as Driver>::error(Error::encode("array", reason)))?;
            let length = i32::try_from(payload.len()).map_err(|_| {
                <Sqlx as Driver>::error(Error::encode("array", "an element is too large"))
            })?;
            buffer.extend_from_slice(&length.to_be_bytes());
            buffer.extend_from_slice(&payload);
        }

        Ok(sqlx::encode::IsNull::No)
    }
}

/// The OID of a type's PostgreSQL binary form, or `None` when Besu cannot write one.
fn element_oid(ty: &besu_core::TypeDef) -> Option<u32> {
    use besu_core::TypeDef;
    Some(match ty {
        TypeDef::Bool => 16,
        TypeDef::Bytes => 17,
        TypeDef::Int8 => 20,
        TypeDef::Int2 => 21,
        TypeDef::Int4 => 23,
        TypeDef::Text => 25,
        TypeDef::Char(_) => 1042,
        TypeDef::VarChar(_) => 1043,
        TypeDef::Float4 => 700,
        TypeDef::Float8 => 701,
        TypeDef::Date => 1082,
        TypeDef::Time | TypeDef::TimeP(_) => 1083,
        TypeDef::Timestamp | TypeDef::TimestampP(_) => 1114,
        TypeDef::TimestampTz | TypeDef::TimestampTzP(_) => 1184,
        TypeDef::TimeTz | TypeDef::TimeTzP(_) => 1266,
        TypeDef::Json => 114,
        TypeDef::Uuid => 2950,
        TypeDef::Jsonb => 3802,
        TypeDef::Bit(_) => 1560,
        TypeDef::VarBit(_) => 1562,
        TypeDef::Interval => 1186,
        TypeDef::Inet => 869,
        TypeDef::Cidr => 650,
        // The unsigned markers are stored as the signed type the dialect declares, except
        // that a 64- or 128-bit one lives in `NUMERIC`.
        TypeDef::UInt(8) => 21,
        TypeDef::UInt(16) => 23,
        TypeDef::UInt(32) => 20,
        TypeDef::UInt(_) | TypeDef::Decimal(_, _) => 1700,
        // PostgreSQL's six built-in range types have fixed OIDs, and an array of one carries
        // the range's OID in its header. A range over anything else has no OID Besu can name.
        TypeDef::Range(inner) => match inner.as_type() {
            TypeDef::Int4 | TypeDef::UInt(16) => 3904,
            TypeDef::Int8 | TypeDef::UInt(32) => 3926,
            TypeDef::Decimal(_, _) => 3906,
            TypeDef::Timestamp | TypeDef::TimestampP(_) => 3908,
            TypeDef::TimestampTz | TypeDef::TimestampTzP(_) => 3910,
            TypeDef::Date => 3912,
            _ => return None,
        },
        TypeDef::Collated(inner, _) => return element_oid(inner),
        // An enum, a domain, a MySQL `SET` or an array of an array is a type Besu has no
        // binary encoder for, and guessing its OID would send bytes the server decodes as
        // something else. A domain's OID is assigned by the server, so it cannot be named
        // here; an array of a domain needs `sql!`.
        TypeDef::Custom(_) | TypeDef::Set(_) | TypeDef::Domain(_) | TypeDef::Array(_) => {
            return None;
        }
        _ => return None,
    })
}

/// Write one array element in its PostgreSQL binary form.
fn encode_element(
    ty: &besu_core::TypeDef,
    value: &Value,
    out: &mut Vec<u8>,
) -> Result<(), &'static str> {
    use besu_core::TypeDef;
    match (ty, value) {
        (_, Value::Null) => return Err("a NULL element has no payload"),
        (TypeDef::Bool, Value::Bool(v)) => out.push(u8::from(*v)),
        (TypeDef::Int2 | TypeDef::UInt(8), _) => {
            let v = element_integer(value)?;
            let v = i16::try_from(v).map_err(|_| "an element does not fit in SMALLINT")?;
            out.extend_from_slice(&v.to_be_bytes());
        }
        (TypeDef::Int4 | TypeDef::UInt(16), _) => {
            let v = element_integer(value)?;
            let v = i32::try_from(v).map_err(|_| "an element does not fit in INTEGER")?;
            out.extend_from_slice(&v.to_be_bytes());
        }
        (TypeDef::Int8 | TypeDef::UInt(32), _) => {
            let v = element_integer(value)?;
            out.extend_from_slice(&v.to_be_bytes());
        }
        (TypeDef::UInt(64) | TypeDef::UInt(128), Value::UInt(v)) => {
            out.extend_from_slice(&numeric_from_text(&v.to_string())?);
        }
        (TypeDef::UInt(128), Value::UInt128(v)) => {
            out.extend_from_slice(&numeric_from_text(&v.to_string())?);
        }
        (TypeDef::Decimal(_, _), Value::Int128(v)) => {
            out.extend_from_slice(&numeric_from_text(&v.to_string())?);
        }
        (TypeDef::Decimal(_, _), Value::Decimal(text)) => {
            out.extend_from_slice(&numeric_from_text(text)?);
        }
        (TypeDef::Decimal(_, _), Value::Int(v)) => {
            out.extend_from_slice(&numeric_from_text(&v.to_string())?);
        }
        // A decimal array element bound from a `String` is text that has already been
        // validated by the conversion; PostgreSQL parses it as NUMERIC.
        (TypeDef::Decimal(_, _), Value::Text(v)) => {
            out.extend_from_slice(&numeric_from_text(v)?);
        }
        (TypeDef::Float4, Value::Float(v)) => out.extend_from_slice(&(*v as f32).to_be_bytes()),
        (TypeDef::Float8, Value::Float(v)) => out.extend_from_slice(&v.to_be_bytes()),
        (TypeDef::Text | TypeDef::Char(_) | TypeDef::VarChar(_), Value::Text(v)) => {
            out.extend_from_slice(v.as_bytes());
        }
        (TypeDef::Bytes, Value::Bytes(v)) => out.extend_from_slice(v),
        (TypeDef::Uuid, Value::Uuid(v)) => out.extend_from_slice(v.as_bytes()),
        (TypeDef::Json, Value::Json(v)) => out.extend_from_slice(v),
        // The binary JSONB form is a one-byte version followed by the document.
        (TypeDef::Jsonb, Value::Json(v)) => {
            out.push(1);
            out.extend_from_slice(v);
        }
        (TypeDef::Interval, Value::Interval(v)) => {
            out.extend_from_slice(&v.microseconds().to_be_bytes());
            out.extend_from_slice(&v.days().to_be_bytes());
            out.extend_from_slice(&v.months().to_be_bytes());
        }
        (TypeDef::Inet, Value::Inet(v)) => {
            write_network(&NetworkValue::inet(v), out);
        }
        (TypeDef::Cidr, Value::Cidr(v)) => {
            write_network(&NetworkValue::cidr(v), out);
        }
        (TypeDef::Bit(_) | TypeDef::VarBit(_), Value::Bit(v)) => {
            let length = i32::try_from(v.len()).map_err(|_| "the bit string is too long")?;
            out.extend_from_slice(&length.to_be_bytes());
            out.extend_from_slice(&v.to_bytes());
        }
        (TypeDef::Date, Value::Date(v)) => {
            let days = v.signed_duration_since(postgres_epoch()).num_days();
            let days = i32::try_from(days).map_err(|_| "the date is out of range")?;
            out.extend_from_slice(&days.to_be_bytes());
        }
        (TypeDef::Time | TypeDef::TimeP(_), Value::Time(v)) => {
            let micros = v.num_seconds_from_midnight() as i64 * 1_000_000
                + i64::from(v.nanosecond() / 1_000);
            out.extend_from_slice(&micros.to_be_bytes());
        }
        (
            TypeDef::Timestamp
            | TypeDef::TimestampP(_)
            | TypeDef::TimestampTz
            | TypeDef::TimestampTzP(_),
            Value::Timestamp(v),
        ) => out.extend_from_slice(&micros_since_epoch(*v)?.to_be_bytes()),
        (
            TypeDef::Timestamp
            | TypeDef::TimestampP(_)
            | TypeDef::TimestampTz
            | TypeDef::TimestampTzP(_),
            Value::TimestampTz(v),
        ) => out.extend_from_slice(&micros_since_epoch(v.naive_utc())?.to_be_bytes()),
        (TypeDef::Range(inner), Value::Range { range, .. }) => {
            write_range(inner.as_type(), range, out)?;
        }
        (TypeDef::Collated(inner, _), _) => return encode_element(inner, value, out),
        (_, _) => return Err("this value does not match its declared element type"),
    }
    Ok(())
}

/// Write a range in PostgreSQL's binary range format.
///
/// `range_send` writes one flag byte and then, for each finite bound, the bound's own
/// encoding behind a four-byte length — exactly as an array element is written. The flags are
/// the empty marker plus one bit for each bound's inclusivity and one for each infinite end.
fn write_range(
    element: &besu_core::TypeDef,
    range: &Range<Value>,
    out: &mut Vec<u8>,
) -> Result<(), &'static str> {
    /// The range is the empty one.
    const EMPTY: u8 = 0x01;
    /// The lower bound is included.
    const LOWER_INCLUDED: u8 = 0x02;
    /// The upper bound is included.
    const UPPER_INCLUDED: u8 = 0x04;
    /// The lower bound is infinite.
    const LOWER_INFINITE: u8 = 0x08;
    /// The upper bound is infinite.
    const UPPER_INFINITE: u8 = 0x10;

    let Some((start, end)) = range.bounds() else {
        out.push(EMPTY);
        return Ok(());
    };

    let mut flags = 0_u8;
    match start {
        besu_core::Bound::Included(_) => flags |= LOWER_INCLUDED,
        besu_core::Bound::Excluded(_) => {}
        besu_core::Bound::Unbounded => flags |= LOWER_INFINITE,
    }
    match end {
        besu_core::Bound::Included(_) => flags |= UPPER_INCLUDED,
        besu_core::Bound::Excluded(_) => {}
        besu_core::Bound::Unbounded => flags |= UPPER_INFINITE,
    }
    out.push(flags);

    for bound in [start, end] {
        let Some(value) = bound.value() else {
            continue;
        };
        let mut payload = Vec::new();
        encode_element(element, value, &mut payload)?;
        let length =
            i32::try_from(payload.len()).map_err(|_| "a range bound is too large to encode")?;
        out.extend_from_slice(&length.to_be_bytes());
        out.extend_from_slice(&payload);
    }
    Ok(())
}

/// An integer value held by an element of a signed integer type.
fn element_integer(value: &Value) -> Result<i64, &'static str> {
    match value {
        Value::Int(v) => Ok(*v),
        Value::UInt(v) => i64::try_from(*v).map_err(|_| "an element does not fit in an integer"),
        _ => Err("this value does not match its declared element type"),
    }
}

/// PostgreSQL counts dates and timestamps from 2000-01-01, not from the Unix epoch.
fn postgres_epoch() -> chrono::NaiveDate {
    // 2000-01-01 is always representable, so the fallback is unreachable; returning the Unix
    // epoch instead of panicking keeps the workspace's no-panic rule.
    chrono::NaiveDate::from_ymd_opt(2000, 1, 1).unwrap_or_default()
}

/// A naive timestamp as whole microseconds since PostgreSQL's epoch.
fn micros_since_epoch(value: chrono::NaiveDateTime) -> Result<i64, &'static str> {
    value
        .signed_duration_since(postgres_epoch().and_time(chrono::NaiveTime::MIN))
        .num_microseconds()
        .ok_or("the timestamp is out of range")
}

/// A bound `NULL` whose type PostgreSQL infers from context.
///
/// sqlx declares a static type for every parameter, and PostgreSQL refuses a `NULL` that is
/// typed `INT4` where a `UUID` is expected. Sending OID `0` (unknown) instead makes the server
/// infer the type from the statement, which is exactly what an untyped [`Value::Null`] means.
#[derive(Debug, Clone, Copy)]
struct UntypedNull;

impl sqlx::Type<SqlxPostgres> for UntypedNull {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::with_oid(sqlx::postgres::types::Oid(0))
    }
}

impl sqlx::Encode<'_, SqlxPostgres> for UntypedNull {
    fn encode_by_ref(
        &self,
        _buffer: &mut PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        Ok(sqlx::encode::IsNull::Yes)
    }
}

/// A text parameter with no declared SQL type.
///
/// PostgreSQL infers a parameter's type from the statement when it is sent untyped (OID 0),
/// which is what an enum's label needs: the type is the column's enum, and a parameter
/// declared `TEXT` is not assignable to an enum without a cast. Every parameter is sent in
/// PostgreSQL's binary format, and the binary form of a text-like type — `TEXT`, `VARCHAR`
/// and an enum alike — is its UTF-8 bytes.
#[derive(Debug, Clone, Copy)]
struct UntypedText<'a>(&'a str);

impl sqlx::Type<SqlxPostgres> for UntypedText<'_> {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::with_oid(sqlx::postgres::types::Oid(0))
    }
}

impl sqlx::Encode<'_, SqlxPostgres> for UntypedText<'_> {
    fn encode_by_ref(
        &self,
        buffer: &mut PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        buffer.extend_from_slice(self.0.as_bytes());
        Ok(sqlx::encode::IsNull::No)
    }

    fn size_hint(&self) -> usize {
        self.0.len()
    }
}

/// An interval encoded into PostgreSQL's `INTERVAL` binary format.
#[derive(Debug, Clone, Copy)]
struct IntervalValue(SqlInterval);

impl sqlx::Type<SqlxPostgres> for IntervalValue {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::with_oid(sqlx::postgres::types::Oid(INTERVAL_OID))
    }
}

impl sqlx::Encode<'_, SqlxPostgres> for IntervalValue {
    fn encode_by_ref(
        &self,
        buffer: &mut PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        // `interval_send` writes the time in microseconds, then whole days, then whole
        // months — in that order, each big-endian.
        buffer.extend_from_slice(&self.0.microseconds().to_be_bytes());
        buffer.extend_from_slice(&self.0.days().to_be_bytes());
        buffer.extend_from_slice(&self.0.months().to_be_bytes());
        Ok(sqlx::encode::IsNull::No)
    }

    fn size_hint(&self) -> usize {
        16
    }
}

/// An `INET` or `CIDR` encoded into PostgreSQL's binary network format.
///
/// The parameter is sent untyped so the server infers `inet` or `cidr` from the column or
/// the comparison: the two types share this binary layout, and declaring one where the other
/// is expected would need a cast the encoder cannot see.
#[derive(Debug, Clone, Copy)]
struct NetworkValue {
    address: core::net::IpAddr,
    prefix: u8,
    cidr: bool,
}

impl NetworkValue {
    fn inet(value: &Inet) -> Self {
        Self::parse(value.as_str(), false)
    }

    fn cidr(value: &Cidr) -> Self {
        Self::parse(value.as_str(), true)
    }

    /// Split the canonical spelling back into its address and prefix.
    ///
    /// The wrappers only hold text they built themselves, so the address always parses; the
    /// unspecified fallback is there to keep a hand-built value from panicking.
    fn parse(text: &str, cidr: bool) -> Self {
        let (address, prefix) = text
            .split_once('/')
            .map_or((text, None), |(address, prefix)| {
                (address, prefix.parse().ok())
            });
        let address = address
            .parse()
            .unwrap_or(core::net::IpAddr::V4(core::net::Ipv4Addr::UNSPECIFIED));
        let prefix = prefix.unwrap_or(if address.is_ipv4() { 32 } else { 128 });
        Self {
            address,
            prefix,
            cidr,
        }
    }
}

impl sqlx::Type<SqlxPostgres> for NetworkValue {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::with_oid(sqlx::postgres::types::Oid(0))
    }
}

impl sqlx::Encode<'_, SqlxPostgres> for NetworkValue {
    fn encode_by_ref(
        &self,
        buffer: &mut PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        write_network(self, buffer);
        Ok(sqlx::encode::IsNull::No)
    }

    fn size_hint(&self) -> usize {
        // Four header bytes plus at most sixteen address bytes.
        20
    }
}

/// Write the five fields of PostgreSQL's network binary format.
fn write_network(value: &NetworkValue, out: &mut Vec<u8>) {
    // `network_send` writes the family (`PGSQL_AF_INET` is 2 and `PGSQL_AF_INET6` is 3),
    // the netmask, whether the value is a `cidr`, the address length, then the address.
    let (family, bytes): (u8, &[u8]) = match &value.address {
        core::net::IpAddr::V4(address) => (2, &address.octets()),
        core::net::IpAddr::V6(address) => (3, &address.octets()),
    };
    out.extend_from_slice(&[
        family,
        value.prefix,
        u8::from(value.cidr),
        bytes.len() as u8,
    ]);
    out.extend_from_slice(bytes);
}

/// A bit string encoded into PostgreSQL's `BIT`/`VARBIT` binary format.
///
/// The parameter is sent untyped: `BIT(n)` and `BIT VARYING(n)` share this layout and differ
/// only in the length they accept, so the column the value is written to decides which one it
/// is.
#[derive(Debug, Clone, Copy)]
struct BitValue<'a>(&'a BitString);

impl sqlx::Type<SqlxPostgres> for BitValue<'_> {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::with_oid(sqlx::postgres::types::Oid(0))
    }
}

impl sqlx::Encode<'_, SqlxPostgres> for BitValue<'_> {
    fn encode_by_ref(
        &self,
        buffer: &mut PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        let length = i32::try_from(self.0.len()).map_err(|_| {
            <Sqlx as Driver>::error(Error::encode("bit", "the bit string is too long"))
        })?;
        buffer.extend_from_slice(&length.to_be_bytes());
        buffer.extend_from_slice(&self.0.to_bytes());
        Ok(sqlx::encode::IsNull::No)
    }

    fn size_hint(&self) -> usize {
        4 + self.0.len().div_ceil(8)
    }
}

/// A range encoded into PostgreSQL's binary range format.
///
/// The parameter is sent untyped, so the server infers which of the six range types it is
/// from the column; the subtype's own encoding is inside the bytes and the server checks it
/// against the type it inferred.
#[derive(Debug, Clone, Copy)]
struct RangeValue<'a> {
    element: &'a besu_core::TypeDef,
    range: &'a Range<Value>,
}

impl sqlx::Type<SqlxPostgres> for RangeValue<'_> {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::with_oid(sqlx::postgres::types::Oid(0))
    }
}

impl sqlx::Encode<'_, SqlxPostgres> for RangeValue<'_> {
    fn encode_by_ref(
        &self,
        buffer: &mut PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        write_range(self.element, self.range, buffer)
            .map_err(|reason| <Sqlx as Driver>::error(Error::encode("range", reason)))?;
        Ok(sqlx::encode::IsNull::No)
    }

    fn size_hint(&self) -> usize {
        // One flag byte, then at most two bounds of a four-byte length and the element.
        1 + self
            .range
            .bounds()
            .map(|(start, end)| {
                [start, end]
                    .into_iter()
                    .filter_map(besu_core::Bound::value)
                    .map(|value| 4 + element_size(value))
                    .sum()
            })
            .unwrap_or(0)
    }
}

/// Bytes bound as an untyped parameter.
///
/// PostGIS's `geometry` and `geography` OIDs are assigned when the extension is created, so
/// there is no stable OID to declare. Sending the EWKB untyped lets the server infer the
/// column's own type instead, exactly as an enum label does.
#[derive(Debug, Clone, Copy)]
struct UntypedBytes<'a>(&'a [u8]);

impl sqlx::Type<SqlxPostgres> for UntypedBytes<'_> {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::with_oid(sqlx::postgres::types::Oid(0))
    }
}

impl sqlx::Encode<'_, SqlxPostgres> for UntypedBytes<'_> {
    fn encode_by_ref(
        &self,
        buffer: &mut PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        buffer.extend_from_slice(self.0);
        Ok(sqlx::encode::IsNull::No)
    }

    fn size_hint(&self) -> usize {
        self.0.len()
    }
}

/// An integer bound as `NUMERIC`, encoded directly into PostgreSQL's binary wire format.
///
/// sqlx ships no `u64`/`u128` support for PostgreSQL because the type system has no
/// unsigned integers, and the type it does implement `NUMERIC` for is `BigDecimal`. Going
/// through `BigDecimal` allocates two decimal-digit vectors per parameter and then walks
/// them again to write the wire format, for a value that is at most ten base-10,000 digits
/// wide. Writing those digits straight into the argument buffer keeps the encode path
/// allocation-free and leaves the parameter's SQL type exactly what it was.
#[derive(Debug, Clone, Copy)]
struct Numeric {
    /// Whether the value is below zero.
    negative: bool,
    /// The absolute value. `u128` is wide enough for every `i128` and `u128`.
    magnitude: u128,
}

/// The number of base-10,000 digits in the widest `NUMERIC` this driver sends.
///
/// `u128::MAX` has 39 decimal digits, which is ten groups of four.
const NUMERIC_GROUPS: usize = 10;

/// PostgreSQL's `NUMERIC` sign for a negative value; zero means positive.
const NUMERIC_SIGN_NEGATIVE: u16 = 0x4000;

impl Numeric {
    /// Build a `NUMERIC` from a signed integer.
    fn signed(value: i128) -> Self {
        Self {
            negative: value < 0,
            magnitude: value.unsigned_abs(),
        }
    }

    /// Build a `NUMERIC` from an unsigned integer.
    fn unsigned(value: u128) -> Self {
        Self {
            negative: false,
            magnitude: value,
        }
    }
}

impl sqlx::Type<SqlxPostgres> for Numeric {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::with_oid(sqlx::postgres::types::Oid(NUMERIC_OID))
    }
}

impl sqlx::Encode<'_, SqlxPostgres> for Numeric {
    fn encode_by_ref(
        &self,
        buffer: &mut PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        // The binary format is a header of `ndigits`, `weight`, `sign` and `dscale`,
        // followed by base-10,000 digits, most significant first.
        let mut groups = [0u16; NUMERIC_GROUPS];
        let mut count = 0;
        let mut rest = self.magnitude;
        while rest != 0 {
            // `count` cannot reach 11: ten groups hold every `u128`.
            groups[count] = (rest % 10_000) as u16;
            rest /= 10_000;
            count += 1;
        }
        // The groups were collected least significant first, and PostgreSQL stores canonical
        // digits, so low-order zero groups are dropped: 10,000 is `[1] * 10000^1`, not
        // `[1, 0]`. Stripping them does not move `weight`, which counts from the most
        // significant digit.
        let mut first = 0;
        while first + 1 < count && groups[first] == 0 {
            first += 1;
        }
        let digits = &groups[first..count];

        let ndigits = digits.len() as i16;
        // `weight` is the exponent of the first digit; a zero has no digits and weight 0.
        let weight = (count as i16).saturating_sub(1);
        let sign = if self.negative {
            NUMERIC_SIGN_NEGATIVE
        } else {
            0
        };

        buffer.extend_from_slice(&ndigits.to_be_bytes());
        buffer.extend_from_slice(&weight.to_be_bytes());
        buffer.extend_from_slice(&sign.to_be_bytes());
        // `dscale` is zero because every value this type carries is an integer.
        buffer.extend_from_slice(&0i16.to_be_bytes());
        // Reversing puts the most significant group first, which is the order of the wire.
        for group in digits.iter().rev() {
            buffer.extend_from_slice(&group.to_be_bytes());
        }

        Ok(sqlx::encode::IsNull::No)
    }

    fn size_hint(&self) -> usize {
        // Header plus at most one two-byte digit per four decimal digits. Ten thousand is
        // just under 2^13.3, so dividing the bit length by 13 never undercounts.
        let digits = match self.magnitude {
            0 => 0,
            magnitude => (128 - magnitude.leading_zeros() as usize).div_ceil(13),
        };
        8 + digits * 2
    }
}

fn add<'q, T>(arguments: &mut PgArguments, value: T) -> Result<(), sqlx::Error>
where
    T: sqlx::Encode<'q, SqlxPostgres> + sqlx::Type<SqlxPostgres> + 'q,
{
    arguments.add(value).map_err(sqlx::Error::Encode)
}

/// The number of bytes encoding `value` will append to an argument buffer.
///
/// Every parameter carries a four-byte length prefix, and the payload sizes are fixed for
/// everything except text, bytes and JSON. Knowing the total up front lets [`arguments`]
/// reserve once instead of growing and copying the buffer as each value is appended.
fn encoded_size(value: &Value) -> usize {
    /// The length prefix that precedes every encoded parameter.
    const LENGTH_PREFIX: usize = 4;

    match value {
        Value::Null => LENGTH_PREFIX,
        Value::Bool(_) => LENGTH_PREFIX + 1,
        Value::Int(_) | Value::Float(_) => LENGTH_PREFIX + 8,
        // `u64::MAX` is twenty decimal digits, which is five base-10,000 groups.
        Value::UInt(_) => LENGTH_PREFIX + 8 + 5 * 2,
        Value::UInt128(_) | Value::Int128(_) => LENGTH_PREFIX + 8 + NUMERIC_GROUPS * 2,
        Value::Text(v) => LENGTH_PREFIX + v.len(),
        Value::EnumLabel(v) => LENGTH_PREFIX + v.len(),
        Value::Bytes(v) => LENGTH_PREFIX + v.len(),
        Value::Uuid(_) => LENGTH_PREFIX + 16,
        Value::Date(_) => LENGTH_PREFIX + 4,
        Value::Time(_) => LENGTH_PREFIX + 8,
        Value::Timestamp(_) | Value::TimestampTz(_) => LENGTH_PREFIX + 8,
        // The JSONB form is a one-byte version plus the document.
        Value::Json(v) => LENGTH_PREFIX + 1 + v.len(),
        // Header plus at most one two-byte digit per four characters of decimal text.
        Value::Decimal(v) => LENGTH_PREFIX + 8 + v.len().div_ceil(4) * 2,
        Value::Interval(_) => LENGTH_PREFIX + 16,
        // Four header bytes and at most sixteen address bytes.
        Value::Inet(_) | Value::Cidr(_) => LENGTH_PREFIX + 20,
        // The int32 bit length plus the packed bits.
        Value::Bit(v) => LENGTH_PREFIX + 4 + v.len().div_ceil(8),
        // One flag byte plus each finite bound's length prefix and element.
        Value::Range { range, .. } => {
            let bounds = range
                .bounds()
                .map(|(start, end)| {
                    [start, end]
                        .into_iter()
                        .filter_map(besu_core::Bound::value)
                        .map(|value| 4 + element_size(value))
                        .sum::<usize>()
                })
                .unwrap_or(0);
            LENGTH_PREFIX + 1 + bounds
        }
        // EWKB is the value itself.
        Value::Geometry(v) => LENGTH_PREFIX + v.ewkb().len(),
        // A dimension and two unused bytes, then four bytes per component.
        Value::Vector(v) => LENGTH_PREFIX + 4 + v.len() * 4,
        // The array header plus a four-byte length and a payload per element; the element
        // sizes are estimated from the values themselves.
        Value::Array { values, .. } => {
            20 + values
                .iter()
                .map(|value| 4 + element_size(value))
                .sum::<usize>()
        }
        // `Value` is `#[non_exhaustive]`. An unknown variant fails to bind before this
        // estimate matters, so reserving nothing for it is harmless.
        _ => 0,
    }
}

/// A rough upper bound on an element's encoded size, for one reservation.
fn element_size(value: &Value) -> usize {
    match value {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Int(_) | Value::Float(_) => 8,
        Value::UInt(_) | Value::UInt128(_) | Value::Int128(_) | Value::Decimal(_) => 8 + 20,
        Value::Text(v) => v.len(),
        Value::EnumLabel(v) => v.len(),
        Value::Bytes(v) => v.len(),
        Value::Uuid(_) => 16,
        Value::Date(_) => 4,
        Value::Time(_) | Value::Timestamp(_) | Value::TimestampTz(_) => 8,
        Value::Json(v) => 1 + v.len(),
        Value::Interval(_) => 16,
        Value::Inet(_) | Value::Cidr(_) => 20,
        Value::Bit(v) => 4 + v.len().div_ceil(8),
        Value::Range { range, .. } => range
            .bounds()
            .map(|(start, end)| {
                1 + [start, end]
                    .into_iter()
                    .filter_map(besu_core::Bound::value)
                    .map(|value| 4 + element_size(value))
                    .sum::<usize>()
            })
            .unwrap_or(1),
        Value::Geometry(v) => v.ewkb().len(),
        Value::Vector(v) => 4 + v.len() * 4,
        // An array of an array is not supported by the encoder, so it adds nothing.
        _ => 0,
    }
}

/// Encode a statement's bound values into the argument buffer `sqlx` sends.
///
/// This is `pub` only so the crate's benchmarks and allocation probes can measure the
/// encoding path without a server; [`crate::encode_arguments`] is the only thing that throws
/// it, and it is hidden from the documentation. Treat it as an implementation detail.
pub fn arguments(sql: &Sql<Postgres>) -> Result<PgArguments, sqlx::Error> {
    let mut arguments = PgArguments::default();
    // The parameter count is known, and so is the encoded width of every value, so one
    // reservation replaces the handful of grow-and-copy steps a fresh buffer would take.
    arguments.reserve(sql.args().len(), sql.args().iter().map(encoded_size).sum());
    for value in sql.args() {
        match value {
            Value::Null => add(&mut arguments, None::<UntypedNull>)?,
            Value::Bool(v) => add(&mut arguments, *v)?,
            Value::Int(v) => add(&mut arguments, *v)?,
            // A `u64` column is `NUMERIC(20,0)`, so the parameter is always `NUMERIC`, even
            // when the value happens to fit in an `i64`. That keeps one SQL type for every
            // value the column can hold.
            Value::UInt(v) => add(&mut arguments, Numeric::unsigned(u128::from(*v)))?,
            Value::UInt128(v) => add(&mut arguments, Numeric::unsigned(*v))?,
            Value::Int128(v) => add(&mut arguments, Numeric::signed(*v))?,
            Value::Float(v) => add(&mut arguments, *v)?,
            Value::Text(v) => add(&mut arguments, v.as_str())?,
            // An enum's label has no type of its own: it is bound untyped so the server
            // infers the enum from the column it is written to or compared with.
            Value::EnumLabel(v) => add(&mut arguments, UntypedText(v.as_str()))?,
            // A `Decimal` column is `NUMERIC`, and PostgreSQL has no implicit `text` to
            // `numeric` cast, so the digits are written in the binary format directly.
            Value::Decimal(v) => add(&mut arguments, NumericText { text: v.as_str() })?,
            Value::Bytes(v) => add(&mut arguments, v.as_slice())?,
            Value::Uuid(v) => add(&mut arguments, *v)?,
            Value::Date(v) => add(&mut arguments, *v)?,
            Value::Time(v) => add(&mut arguments, *v)?,
            Value::Timestamp(v) => add(&mut arguments, *v)?,
            Value::TimestampTz(v) => add(&mut arguments, *v)?,
            Value::Interval(v) => add(&mut arguments, IntervalValue(*v))?,
            Value::Inet(v) => add(&mut arguments, NetworkValue::inet(v))?,
            Value::Cidr(v) => add(&mut arguments, NetworkValue::cidr(v))?,
            Value::Bit(v) => add(&mut arguments, BitValue(v))?,
            // A range is bound untyped: the server infers which of the six range types it is
            // from the column, and the subtype's encoding travels inside the bytes.
            Value::Range { element, range } => add(&mut arguments, RangeValue { element, range })?,
            // PostGIS's OIDs are extension-defined, so the EWKB goes over as an untyped
            // parameter and the server infers `geometry` or `geography`.
            Value::Geometry(v) => add(&mut arguments, UntypedBytes(v.ewkb()))?,
            // `pgvector`'s OID is extension-defined too, so the value goes over untyped and
            // the server infers the column's `vector(n)` type. Every sqlx parameter is sent
            // in binary format, so what travels is `vector_recv`'s form — a dimension and
            // big-endian components — not the `[1,2,3]` text a SQL literal would use.
            Value::Vector(v) => {
                let bytes = v.to_binary();
                add(&mut arguments, UntypedBytes(&bytes))?;
            }
            Value::Json(v) => {
                // The document is already JSON text. Validating it in place and binding the
                // borrowed `RawValue` lets sqlx copy those bytes into its buffer, instead of
                // parsing them into a `serde_json::Value` tree only to serialise the tree
                // back to the same bytes. The bound type is still `JSONB`, because it is
                // `Json<T>`'s type for every `T`, so the prepared statement is unchanged.
                let json: &serde_json::value::RawValue =
                    serde_json::from_slice(v).map_err(|err| {
                        <Sqlx as Driver>::error(Error::encode("json", err.to_string()))
                    })?;
                add(&mut arguments, sqlx::types::Json(json))?;
            }
            Value::Array { element, values } => add(
                &mut arguments,
                ArrayValue {
                    element,
                    values: values.as_slice(),
                },
            )?,
            // `Value` is `#[non_exhaustive]`: a future variant must fail loudly rather than
            // silently bind `NULL`.
            other => {
                return Err(<Sqlx as Driver>::error(Error::encode(
                    other.display(),
                    "this driver does not know how to bind this value",
                )));
            }
        }
    }
    Ok(arguments)
}

impl Driver for Sqlx {
    type Dialect = Postgres;
    type Error = sqlx::Error;
    type Row = PgRow;
    type Output = PgQueryResult;

    fn error(err: Error) -> Self::Error {
        sqlx::Error::Protocol(err.to_string())
    }

    fn row_len(row: &Self::Row) -> usize {
        row.len()
    }

    fn is_null(row: &Self::Row, index: usize) -> bool {
        row.try_get_raw(index)
            .map(|value| ValueRef::is_null(&value))
            .unwrap_or(true)
    }

    fn column_names(row: &Self::Row) -> Option<Vec<String>> {
        Some(
            row.columns()
                .iter()
                .map(|column| column.name().to_owned())
                .collect(),
        )
    }

    fn decode_text(row: &Self::Row, index: usize) -> Result<String, Self::Error> {
        // Unchecked, because the type this reads from is not `TEXT`: an enum column is a
        // named type whose binary form is still the label's UTF-8 bytes.
        row.try_get_unchecked(index)
    }

    async fn query(&self, sql: &Sql<Postgres>) -> Result<Vec<Self::Row>, Self::Error> {
        let arguments = arguments(sql)?;
        sqlx::query_with(sql.sql(), arguments)
            .fetch_all(&self.pool)
            .await
    }

    async fn execute(&self, sql: &Sql<Postgres>) -> Result<Self::Output, Self::Error> {
        let arguments = arguments(sql)?;
        sqlx::query_with(sql.sql(), arguments)
            .execute(&self.pool)
            .await
    }

    async fn query_stream<'a>(
        &'a self,
        sql: &'a Sql<Postgres>,
    ) -> Result<besu_core::RowStream<'a, Self>, Self::Error> {
        let arguments = arguments(sql)?;
        Ok(sqlx::query_with(sql.sql(), arguments).fetch(&self.pool))
    }
}

impl besu::SchemaDriver for Sqlx {
    type Ddl = crate::PostgresDdl;

    async fn introspect(&self) -> Result<besu::schema::Schema, Self::Error> {
        let db = besu::Database::new(self.clone());
        crate::introspect::introspect(&db).await
    }
}

impl TransactionDriver for Sqlx {
    type Transaction<'a> = sqlx::Transaction<'a, SqlxPostgres>;

    async fn begin<'a>(&'a self) -> Result<Self::Transaction<'a>, Self::Error> {
        self.pool.begin().await
    }

    async fn commit<'a>(tx: Self::Transaction<'a>) -> Result<(), Self::Error> {
        tx.commit().await
    }

    async fn rollback<'a>(tx: Self::Transaction<'a>) -> Result<(), Self::Error> {
        tx.rollback().await
    }

    async fn txn_query<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &Sql<Postgres>,
    ) -> Result<Vec<Self::Row>, Self::Error> {
        let arguments = arguments(sql)?;
        sqlx::query_with(sql.sql(), arguments)
            .fetch_all(&mut **tx)
            .await
    }

    async fn txn_execute<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &Sql<Postgres>,
    ) -> Result<Self::Output, Self::Error> {
        let arguments = arguments(sql)?;
        sqlx::query_with(sql.sql(), arguments)
            .execute(&mut **tx)
            .await
    }

    async fn txn_query_stream<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &'c Sql<Postgres>,
    ) -> Result<besu_core::RowStream<'c, Self>, Self::Error> {
        let arguments = arguments(sql)?;
        Ok(sqlx::query_with(sql.sql(), arguments).fetch(&mut **tx))
    }
}

// ---------------------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------------------

fn decode_via_sqlx<T>(row: &PgRow, offset: &mut usize) -> Result<T, sqlx::Error>
where
    T: for<'r> sqlx::Decode<'r, SqlxPostgres> + sqlx::Type<SqlxPostgres>,
{
    let index = *offset;
    *offset += 1;
    let value = row.try_get_raw(index)?;
    // A domain's declared type is the domain, so the checked path refuses it even though its
    // wire format is the base type's. The base type is what the Rust type was written for, so
    // reading unchecked is exactly right.
    if is_domain(&value) {
        row.try_get_unchecked(index)
    } else {
        row.try_get(index)
    }
}

fn decode_integer<T>(row: &PgRow, offset: &mut usize) -> Result<T, sqlx::Error>
where
    T: TryFrom<i64>,
{
    let index = *offset;
    *offset += 1;
    // Go through `i64`: sqlx refuses to decode an `INT4` column straight into `i16`, because
    // it compares the declared SQL type against `T`'s own type.
    let value: i64 = row.try_get_unchecked(index)?;
    T::try_from(value).map_err(|_| {
        <Sqlx as Driver>::error(Error::decode(
            index,
            format!(
                "value is out of range for `{}`",
                core::any::type_name::<T>()
            ),
        ))
    })
}

macro_rules! impl_decode {
    ($($ty:ty),* $(,)?) => {
        $(
            impl Decode<Sqlx> for $ty {
                fn decode(row: &PgRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
                    decode_via_sqlx(row, offset)
                }
            }
        )*
    };
}

impl_decode!(
    bool,
    i64,
    f64,
    Vec<u8>,
    uuid::Uuid,
    chrono::NaiveDate,
    chrono::NaiveTime,
    chrono::NaiveDateTime,
    chrono::DateTime<chrono::Utc>,
);

impl Decode<Sqlx> for String {
    fn decode(row: &PgRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;
        let value = row.try_get_raw(index)?;
        // A `NUMERIC` column is the exact decimal text, and sqlx will not hand it to a
        // `String` because the declared type is not a string type. Reading its wire format
        // is what makes `Decimal` usable end to end.
        if type_is(&value, NUMERIC_OID) {
            *offset += 1;
            return decode_numeric_text(value, index);
        }
        decode_via_sqlx(row, offset)
    }
}

macro_rules! impl_decode_integer {
    ($($ty:ty),* $(,)?) => {
        $(
            impl Decode<Sqlx> for $ty {
                fn decode(row: &PgRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
                    decode_integer(row, offset)
                }
            }
        )*
    };
}

impl_decode_integer!(i8, i16, i32, u8, u16, u32);

impl Decode<Sqlx> for u64 {
    fn decode(row: &PgRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;
        *offset += 1;

        let value = row.try_get_raw(index)?;
        if type_is(&value, NUMERIC_OID) {
            // `UInt(64)` is declared `NUMERIC(20,0)`. Read its wire format directly so the
            // decode path does not depend on a decimal type beyond what encoding needs.
            let value = decode_numeric_unsigned(value, index)?;
            return u64::try_from(value).map_err(|_| {
                <Sqlx as Driver>::error(Error::decode(
                    index,
                    "the NUMERIC value is too large for `u64`",
                ))
            });
        }

        // A `BIGINT` (or any narrower integer type) reads through `i64` and then range-checks.
        let value: i64 = row.try_get_unchecked(index)?;
        u64::try_from(value).map_err(|_| {
            <Sqlx as Driver>::error(Error::decode(
                index,
                "a negative PostgreSQL integer cannot be decoded as `u64`",
            ))
        })
    }
}

impl Decode<Sqlx> for u128 {
    fn decode(row: &PgRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;
        *offset += 1;

        let value = row.try_get_raw(index)?;
        if type_is(&value, NUMERIC_OID) {
            // `UInt(128)` is declared `NUMERIC(39,0)`, so the `NUMERIC` reader returns the
            // full width without narrowing to `u64` first.
            return decode_numeric_unsigned(value, index);
        }

        // An integer column cannot hold a value above `i64::MAX` on PostgreSQL, but reading
        // one this way still lets a hand-written `SELECT 1::bigint` decode into a `u128`.
        let value: i64 = row.try_get_unchecked(index)?;
        u128::try_from(value).map_err(|_| {
            <Sqlx as Driver>::error(Error::decode(
                index,
                "a negative PostgreSQL integer cannot be decoded as `u128`",
            ))
        })
    }
}

impl Decode<Sqlx> for i128 {
    fn decode(row: &PgRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;
        *offset += 1;

        let value = row.try_get_raw(index)?;
        if type_is(&value, NUMERIC_OID) {
            return decode_numeric_signed(value, index);
        }

        let value: i64 = row.try_get_unchecked(index)?;
        Ok(i128::from(value))
    }
}

/// Decode a non-negative integral `NUMERIC` into a `u128`.
fn decode_numeric_unsigned(value: PgValueRef<'_>, index: usize) -> Result<u128, sqlx::Error> {
    let malformed = |reason: &str| <Sqlx as Driver>::error(Error::decode(index, reason));

    match value.format() {
        PgValueFormat::Text => value
            .as_str()
            .map_err(|err| malformed(&err.to_string()))?
            .trim()
            .parse()
            .map_err(|_| malformed("the NUMERIC value is not an integer that fits in `u128`")),
        PgValueFormat::Binary => {
            let bytes = value
                .as_bytes()
                .map_err(|err| malformed(&err.to_string()))?;
            let (negative, magnitude) = decode_numeric_binary(bytes).map_err(malformed)?;
            if negative {
                Err(malformed(
                    "a negative NUMERIC cannot be decoded as an unsigned integer",
                ))
            } else {
                Ok(magnitude)
            }
        }
    }
}

/// Decode an integral `NUMERIC` into an `i128`.
fn decode_numeric_signed(value: PgValueRef<'_>, index: usize) -> Result<i128, sqlx::Error> {
    let malformed = |reason: &str| <Sqlx as Driver>::error(Error::decode(index, reason));

    match value.format() {
        PgValueFormat::Text => value
            .as_str()
            .map_err(|err| malformed(&err.to_string()))?
            .trim()
            .parse()
            .map_err(|_| malformed("the NUMERIC value is not an integer that fits in `i128`")),
        PgValueFormat::Binary => {
            let bytes = value
                .as_bytes()
                .map_err(|err| malformed(&err.to_string()))?;
            let (negative, magnitude) = decode_numeric_binary(bytes).map_err(malformed)?;
            if negative {
                // `0 - magnitude` is representable even at `i128::MIN`.
                0i128
                    .checked_sub_unsigned(magnitude)
                    .ok_or_else(|| malformed("the NUMERIC value is too small for `i128`"))
            } else {
                i128::try_from(magnitude)
                    .map_err(|_| malformed("the NUMERIC value is too large for `i128`"))
            }
        }
    }
}

/// Decode the binary wire format of an integral `NUMERIC` into its sign and magnitude.
///
/// The magnitude is a `u128`, which is wide enough for the whole `NUMERIC(39,0)` range. A
/// caller that needs a narrower integer range-checks the result.
fn decode_numeric_binary(bytes: &[u8]) -> Result<(bool, u128), &'static str> {
    let header = bytes.get(..8).ok_or("malformed NUMERIC value")?;
    let ndigits = u16::from_be_bytes([header[0], header[1]]);
    let weight = i16::from_be_bytes([header[2], header[3]]);
    let sign = u16::from_be_bytes([header[4], header[5]]);
    let scale = i16::from_be_bytes([header[6], header[7]]);

    // `0x4000` is negative; `0xC000` and `0xD000` are NaN and the infinities.
    let negative = match sign {
        0x0000 => false,
        0x4000 => true,
        _ => return Err("a non-finite NUMERIC cannot be decoded as an integer"),
    };
    if scale != 0 {
        return Err("a NUMERIC with a fractional part cannot be decoded as an integer");
    }
    if bytes.len() < 8
        || (bytes.len() - 8) % 2 != 0
        || usize::from(ndigits) != (bytes.len() - 8) / 2
    {
        return Err("malformed NUMERIC value");
    }
    // A zero has no digits to fold in, and its weight carries no information.
    if ndigits == 0 {
        return Ok((negative, 0));
    }

    // Fold the digits in most significant first. That reads them as though the leading digit
    // had weight `ndigits - 1`, which is true for every value a `NUMERIC(p,0)` column holds;
    // a value PostgreSQL stores with a gap between its weight and its digits needs one
    // further shift, which is cheaper than raising 10,000 to a power per digit.
    let mut value: u128 = 0;
    for position in 0..usize::from(ndigits) {
        let offset = 8 + position * 2;
        let digit = u128::from(u16::from_be_bytes([bytes[offset], bytes[offset + 1]]));
        value = value
            .checked_mul(10_000)
            .and_then(|value| value.checked_add(digit))
            .ok_or("the NUMERIC value is too large for a 128-bit integer")?;
    }

    // `1e8` is one digit with weight 2, for example. A negative gap means a digit sits below
    // the value's scale, which no integer column produces.
    let gap = i32::from(weight) - (i32::from(ndigits) - 1);
    let gap = u32::try_from(gap).map_err(|_| "malformed NUMERIC value")?;
    if gap != 0 {
        let factor = 10_000_u128
            .checked_pow(gap)
            .ok_or("the NUMERIC value is too large for a 128-bit integer")?;
        value = value
            .checked_mul(factor)
            .ok_or("the NUMERIC value is too large for a 128-bit integer")?;
    }

    Ok((negative, value))
}

impl Decode<Sqlx> for f32 {
    fn decode(row: &PgRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;
        *offset += 1;

        // `REAL` is four bytes and `DOUBLE PRECISION` is eight, and a `f32` can legitimately
        // come back as either, so let the column type pick the decoder.
        let value = row.try_get_raw(index)?;
        if type_is(&value, FLOAT4_OID) {
            row.try_get_unchecked(index)
        } else {
            let value: f64 = row.try_get_unchecked(index)?;
            Ok(value as f32)
        }
    }
}

impl Decode<Sqlx> for serde_json::Value {
    fn decode(row: &PgRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;
        let value = row.try_get_raw(index)?;
        // `sqlx`'s JSON wrapper types are checked against `json`/`jsonb`, so a domain over
        // either has to be read from its base type's bytes directly.
        if is_domain(&value) {
            *offset += 1;
            let jsonb = type_is(&value, 3802);
            let text = match value.format() {
                PgValueFormat::Text => value
                    .as_str()
                    .map_err(|err| <Sqlx as Driver>::error(Error::decode(index, err.to_string())))?
                    .to_owned(),
                PgValueFormat::Binary => {
                    let bytes = value.as_bytes().map_err(|err| {
                        <Sqlx as Driver>::error(Error::decode(index, err.to_string()))
                    })?;
                    let bytes = if jsonb {
                        match bytes.split_first() {
                            Some((1, rest)) => rest,
                            _ => {
                                return Err(<Sqlx as Driver>::error(Error::decode(
                                    index,
                                    "malformed JSONB value",
                                )));
                            }
                        }
                    } else {
                        bytes
                    };
                    core::str::from_utf8(bytes)
                        .map_err(|_| {
                            <Sqlx as Driver>::error(Error::decode(
                                index,
                                "the JSON value is not valid UTF-8",
                            ))
                        })?
                        .to_owned()
                }
            };
            return serde_json::from_str(&text).map_err(|err| {
                <Sqlx as Driver>::error(Error::decode(index, format!("invalid JSON: {err}")))
            });
        }
        let value: sqlx::types::Json<serde_json::Value> = decode_via_sqlx(row, offset)?;
        Ok(value.0)
    }
}

/// Decode a `INTERVAL` from PostgreSQL's binary format: microseconds, days, months.
fn decode_interval_binary(bytes: &[u8]) -> Result<SqlInterval, &'static str> {
    let bytes: [u8; 16] = bytes.try_into().map_err(|_| "malformed INTERVAL value")?;
    let microseconds = i64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]);
    let days = i32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let months = i32::from_be_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    Ok(SqlInterval::new(months, days, microseconds))
}

impl Decode<Sqlx> for SqlInterval {
    fn decode(row: &PgRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;
        *offset += 1;
        let malformed = |reason: &str| <Sqlx as Driver>::error(Error::decode(index, reason));

        let value = row.try_get_raw(index)?;
        match value.format() {
            PgValueFormat::Text => value
                .as_str()
                .map_err(|err| malformed(&err.to_string()))?
                .parse()
                .map_err(|err: besu_core::ParseValueError| malformed(&err.to_string())),
            PgValueFormat::Binary => {
                let bytes = value
                    .as_bytes()
                    .map_err(|err| malformed(&err.to_string()))?;
                decode_interval_binary(bytes).map_err(malformed)
            }
        }
    }
}

/// Decode a `INET` or `CIDR` from PostgreSQL's binary network format into canonical text.
fn decode_network_binary(bytes: &[u8]) -> Result<String, &'static str> {
    let header = bytes.get(..4).ok_or("malformed network value")?;
    let family = header[0];
    let prefix = header[1];
    let address = bytes.get(4..).ok_or("malformed network value")?;
    let address = match (family, address) {
        (2, [a, b, c, d]) => core::net::IpAddr::V4(core::net::Ipv4Addr::new(*a, *b, *c, *d)),
        (3, bytes) => {
            let bytes: [u8; 16] = bytes.try_into().map_err(|_| "malformed network value")?;
            core::net::IpAddr::V6(core::net::Ipv6Addr::from(bytes))
        }
        _ => return Err("malformed network value"),
    };
    Ok(format!("{address}/{prefix}"))
}

impl Decode<Sqlx> for Inet {
    fn decode(row: &PgRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;
        *offset += 1;
        let malformed = |reason: &str| <Sqlx as Driver>::error(Error::decode(index, reason));

        let value = row.try_get_raw(index)?;
        let text = match value.format() {
            PgValueFormat::Text => value
                .as_str()
                .map_err(|err| malformed(&err.to_string()))?
                .to_owned(),
            PgValueFormat::Binary => {
                let bytes = value
                    .as_bytes()
                    .map_err(|err| malformed(&err.to_string()))?;
                let text = decode_network_binary(bytes).map_err(malformed)?;
                // The unpacked spelling always carries a prefix; `Inet` canonicalises it the
                // way the server prints it, dropping `/32` and `/128`.
                text
            }
        };
        Self::new(&text).map_err(|err| malformed(&err.to_string()))
    }
}

impl Decode<Sqlx> for Cidr {
    fn decode(row: &PgRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;
        *offset += 1;
        let malformed = |reason: &str| <Sqlx as Driver>::error(Error::decode(index, reason));

        let value = row.try_get_raw(index)?;
        let text = match value.format() {
            PgValueFormat::Text => value
                .as_str()
                .map_err(|err| malformed(&err.to_string()))?
                .to_owned(),
            PgValueFormat::Binary => {
                let bytes = value
                    .as_bytes()
                    .map_err(|err| malformed(&err.to_string()))?;
                decode_network_binary(bytes).map_err(malformed)?
            }
        };
        Self::new(&text).map_err(|err| malformed(&err.to_string()))
    }
}

impl Decode<Sqlx> for BitString {
    fn decode(row: &PgRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;
        *offset += 1;
        let malformed = |reason: &str| <Sqlx as Driver>::error(Error::decode(index, reason));

        let value = row.try_get_raw(index)?;
        match value.format() {
            PgValueFormat::Text => {
                let text = value.as_str().map_err(|err| malformed(&err.to_string()))?;
                Self::new(text).map_err(|err| malformed(&err.to_string()))
            }
            PgValueFormat::Binary => {
                let bytes = value
                    .as_bytes()
                    .map_err(|err| malformed(&err.to_string()))?;
                let header = bytes
                    .get(..4)
                    .ok_or_else(|| malformed("malformed BIT value"))?;
                let length = i32::from_be_bytes([header[0], header[1], header[2], header[3]]);
                let length =
                    usize::try_from(length).map_err(|_| malformed("the bit length is negative"))?;
                Self::from_bytes(&bytes[4..], length).map_err(|err| malformed(&err.to_string()))
            }
        }
    }
}

/// A type that can be the subtype of a PostgreSQL range.
///
/// Only the six built-in range types are modelled, and each one's element has a fixed OID
/// that the decoder needs because the range wire format does not repeat it.
trait RangeElement: ArrayElement {
    /// The OID of the element type, as an array of it would report.
    const ELEMENT_OID: u32;
}

impl RangeElement for i32 {
    const ELEMENT_OID: u32 = 23;
}

impl RangeElement for i64 {
    const ELEMENT_OID: u32 = 20;
}

impl RangeElement for String {
    const ELEMENT_OID: u32 = NUMERIC_OID;
}

impl RangeElement for chrono::NaiveDateTime {
    const ELEMENT_OID: u32 = 1114;
}

impl RangeElement for chrono::DateTime<chrono::Utc> {
    const ELEMENT_OID: u32 = 1184;
}

impl RangeElement for chrono::NaiveDate {
    const ELEMENT_OID: u32 = 1082;
}

/// Decode a range from PostgreSQL's binary format: a flag byte, then each finite bound as a
/// four-byte length and the element's own encoding.
fn decode_range_binary<T: RangeElement>(bytes: &[u8]) -> Result<Range<T>, &'static str> {
    /// The range is the empty one.
    const EMPTY: u8 = 0x01;
    /// The lower bound is included.
    const LOWER_INCLUDED: u8 = 0x02;
    /// The upper bound is included.
    const UPPER_INCLUDED: u8 = 0x04;
    /// The lower bound is infinite.
    const LOWER_INFINITE: u8 = 0x08;
    /// The upper bound is infinite.
    const UPPER_INFINITE: u8 = 0x10;
    /// Either bound may be `NULL`, which a Rust range cannot hold.
    const ANY_NULL: u8 = 0x20 | 0x40;

    let (flags, mut rest) = bytes.split_first().ok_or("malformed range value")?;
    if flags & ANY_NULL != 0 {
        return Err("a range with a NULL bound does not fit in `Range`");
    }
    if flags & EMPTY != 0 {
        return Ok(Range::empty());
    }

    let decode_bound = |rest: &mut &[u8],
                        infinite: u8,
                        included: u8|
     -> Result<besu_core::Bound<T>, &'static str> {
        if flags & infinite != 0 {
            return Ok(besu_core::Bound::Unbounded);
        }
        let length = rest.get(..4).ok_or("malformed range value")?;
        let length = i32::from_be_bytes([length[0], length[1], length[2], length[3]]);
        let length = usize::try_from(length).map_err(|_| "malformed range value")?;
        // Step past the length word before reading the payload.
        *rest = rest.get(4..).ok_or("malformed range value")?;
        let (payload, tail) = rest
            .split_at_checked(length)
            .ok_or("malformed range value")?;
        *rest = tail;
        let value = T::decode_element(T::ELEMENT_OID, payload)?;
        Ok(if flags & included != 0 {
            besu_core::Bound::Included(value)
        } else {
            besu_core::Bound::Excluded(value)
        })
    };
    let start = decode_bound(&mut rest, LOWER_INFINITE, LOWER_INCLUDED)?;
    let end = decode_bound(&mut rest, UPPER_INFINITE, UPPER_INCLUDED)?;
    Ok(Range::new(start, end))
}

impl<T: RangeElement> Decode<Sqlx> for Range<T> {
    fn decode(row: &PgRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;
        *offset += 1;
        let malformed = |reason: &str| <Sqlx as Driver>::error(Error::decode(index, reason));

        let value = row.try_get_raw(index)?;
        match value.format() {
            PgValueFormat::Text => {
                let text = value.as_str().map_err(|err| malformed(&err.to_string()))?;
                Range::parse_with(text.trim(), T::decode_text)
                    .map_err(|err| malformed(&err.to_string()))
            }
            PgValueFormat::Binary => {
                let bytes = value
                    .as_bytes()
                    .map_err(|err| malformed(&err.to_string()))?;
                decode_range_binary::<T>(bytes).map_err(malformed)
            }
        }
    }
}

impl<T: RangeElement> ArrayElement for Range<T> {
    fn decode_element(_element: u32, bytes: &[u8]) -> Result<Self, &'static str> {
        decode_range_binary::<T>(bytes)
    }

    fn decode_text(text: &str) -> Result<Self, &'static str> {
        Range::parse_with(text.trim(), T::decode_text)
            .map_err(|_| "an array element is not a range")
    }
}

impl Decode<Sqlx> for Geometry {
    fn decode(row: &PgRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;
        *offset += 1;
        let malformed = |reason: &str| <Sqlx as Driver>::error(Error::decode(index, reason));

        let value = row.try_get_raw(index)?;
        match value.format() {
            // PostGIS's output function writes the EWKB as hex, not as WKT.
            PgValueFormat::Text => {
                let text = value.as_str().map_err(|err| malformed(&err.to_string()))?;
                Self::from_hex(text).map_err(|err| malformed(&err.to_string()))
            }
            PgValueFormat::Binary => {
                let bytes = value
                    .as_bytes()
                    .map_err(|err| malformed(&err.to_string()))?;
                Ok(Self::from_ewkb_unchecked(bytes.to_vec()))
            }
        }
    }
}

impl Decode<Sqlx> for Vector {
    fn decode(row: &PgRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;
        *offset += 1;
        let malformed = |reason: &str| <Sqlx as Driver>::error(Error::decode(index, reason));

        let value = row.try_get_raw(index)?;
        match value.format() {
            PgValueFormat::Text => {
                let text = value.as_str().map_err(|err| malformed(&err.to_string()))?;
                text.trim()
                    .parse()
                    .map_err(|err: besu_core::ParseValueError| malformed(&err.to_string()))
            }
            PgValueFormat::Binary => {
                let bytes = value
                    .as_bytes()
                    .map_err(|err| malformed(&err.to_string()))?;
                Self::from_binary(bytes).map_err(|err| malformed(&err.to_string()))
            }
        }
    }
}

/// Decode a `NUMERIC` as its exact decimal text.
fn decode_numeric_text(value: PgValueRef<'_>, index: usize) -> Result<String, sqlx::Error> {
    let malformed = |reason: &str| <Sqlx as Driver>::error(Error::decode(index, reason));

    match value.format() {
        PgValueFormat::Text => Ok(value
            .as_str()
            .map_err(|err| malformed(&err.to_string()))?
            .trim()
            .to_owned()),
        PgValueFormat::Binary => {
            let bytes = value
                .as_bytes()
                .map_err(|err| malformed(&err.to_string()))?;
            decode_numeric_decimal(bytes).map_err(malformed)
        }
    }
}

/// Turn the binary `NUMERIC` wire format into its decimal text.
///
/// `dscale` is the number of digits the column displays after the point, so it is what makes
/// `1.90` come back as `1.90` rather than `1.9`: the digits themselves are stored in groups
/// of four and carry no trailing zeros.
fn decode_numeric_decimal(bytes: &[u8]) -> Result<String, &'static str> {
    let header = bytes.get(..8).ok_or("malformed NUMERIC value")?;
    let ndigits = u16::from_be_bytes([header[0], header[1]]);
    let weight = i16::from_be_bytes([header[2], header[3]]);
    let sign = u16::from_be_bytes([header[4], header[5]]);
    let dscale = i16::from_be_bytes([header[6], header[7]]);

    match sign {
        0x0000 | 0x4000 => {}
        0xC000 => return Ok("NaN".to_owned()),
        0xD000 => return Ok("Infinity".to_owned()),
        0xF000 => return Ok("-Infinity".to_owned()),
        _ => return Err("malformed NUMERIC value"),
    }
    if dscale < 0 {
        return Err("malformed NUMERIC value");
    }
    if bytes.len() < 8
        || (bytes.len() - 8) % 2 != 0
        || usize::from(ndigits) != (bytes.len() - 8) / 2
    {
        return Err("malformed NUMERIC value");
    }

    let mut groups: Vec<u16> = Vec::with_capacity(usize::from(ndigits));
    for position in 0..usize::from(ndigits) {
        let offset = 8 + position * 2;
        let group = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
        // Every digit is base 10,000; anything larger is not a NUMERIC this decoder can read.
        if group >= 10_000 {
            return Err("malformed NUMERIC value");
        }
        groups.push(group);
    }

    // The group at exponent `e` is stored `weight - e` positions from the start.
    let group_at = |exponent: i32| -> Option<u16> {
        let position = i32::from(weight) - exponent;
        if position < 0 {
            return None;
        }
        groups.get(usize::try_from(position).ok()?).copied()
    };

    // The integer part runs from the most significant group down to exponent zero.
    let mut integer = String::new();
    if weight >= 0 {
        for exponent in (0..=i32::from(weight)).rev() {
            push_group(&mut integer, group_at(exponent).unwrap_or(0));
        }
        let trimmed = integer.trim_start_matches('0');
        integer = if trimmed.is_empty() {
            "0".to_owned()
        } else {
            trimmed.to_owned()
        };
    } else {
        integer.push('0');
    }

    let scale = usize::try_from(dscale).unwrap_or(0);
    let mut fraction = String::new();
    let groups_needed = scale.div_ceil(4);
    for index in 0..groups_needed {
        let exponent = -(i32::try_from(index).unwrap_or(0)) - 1;
        push_group(&mut fraction, group_at(exponent).unwrap_or(0));
    }
    fraction.truncate(scale);
    while fraction.len() < scale {
        fraction.push('0');
    }

    let negative = sign == 0x4000;
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    out.push_str(&integer);
    if !fraction.is_empty() {
        out.push('.');
        out.push_str(&fraction);
    }
    Ok(out)
}

fn push_group(out: &mut String, group: u16) {
    for position in (0..4).rev() {
        let digit = (group / 10_u16.pow(position)) % 10;
        out.push(char::from(b'0' + digit as u8));
    }
}

/// A type that can be read out of one element of a PostgreSQL array.
trait ArrayElement: Sized {
    /// Decode one element from its binary form, given the element OID in the header.
    fn decode_element(element: u32, bytes: &[u8]) -> Result<Self, &'static str>;

    /// Decode one element from the text form of an array literal.
    ///
    /// sqlx asks the server for a type it does not know in text form; it requests binary
    /// results for everything else, so this is the fallback rather than the common path. The
    /// Rust type is known here, so the element does not need the header's OID.
    fn decode_text(text: &str) -> Result<Self, &'static str>;
}

fn decode_array<T: ArrayElement>(row: &PgRow, offset: &mut usize) -> Result<Vec<T>, sqlx::Error> {
    let index = *offset;
    *offset += 1;
    let malformed = |reason: &str| <Sqlx as Driver>::error(Error::decode(index, reason));

    let value = row.try_get_raw(index)?;
    if value.format() == PgValueFormat::Text {
        let text = value.as_str().map_err(|err| malformed(&err.to_string()))?;
        return decode_text_array::<T>(text).map_err(malformed);
    }
    let bytes = value
        .as_bytes()
        .map_err(|err| malformed(&err.to_string()))?;

    let header = bytes
        .get(..12)
        .ok_or_else(|| malformed("malformed array value"))?;
    let dimensions = i32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    // An empty array is written with no dimensions at all, not one dimension of length zero.
    if dimensions == 0 {
        return Ok(Vec::new());
    }
    if dimensions != 1 {
        return Err(malformed(
            "only one-dimensional arrays decode into a `Vec`; an array of arrays needs `sql!`",
        ));
    }
    let element = u32::from_be_bytes([header[8], header[9], header[10], header[11]]);

    // One `(length, lower_bound)` pair per dimension, before the elements.
    let dimension = bytes
        .get(12..20)
        .ok_or_else(|| malformed("malformed array value"))?;
    let count = i32::from_be_bytes([dimension[0], dimension[1], dimension[2], dimension[3]]);
    if count < 0 {
        return Err(malformed("malformed array value"));
    }

    let mut out = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
    let mut rest = &bytes[20..];
    while !rest.is_empty() {
        let length = rest
            .get(..4)
            .ok_or_else(|| malformed("malformed array value"))?;
        let length = i32::from_be_bytes([length[0], length[1], length[2], length[3]]);
        rest = &rest[4..];
        if length < 0 {
            return Err(malformed(
                "the array contains a NULL element, which a `Vec<T>` cannot hold",
            ));
        }
        let length = usize::try_from(length).map_err(|_| malformed("malformed array value"))?;
        let (element_bytes, tail) = rest
            .split_at_checked(length)
            .ok_or_else(|| malformed("malformed array value"))?;
        out.push(T::decode_element(element, element_bytes).map_err(malformed)?);
        rest = tail;
    }
    Ok(out)
}

/// Read an integer element of a given width from the array's bytes.
fn array_integer(element: u32, bytes: &[u8]) -> Result<i64, &'static str> {
    match element {
        21 => match bytes {
            [a, b] => Ok(i64::from(i16::from_be_bytes([*a, *b]))),
            _ => Err("malformed SMALLINT element"),
        },
        23 => match bytes {
            [a, b, c, d] => Ok(i64::from(i32::from_be_bytes([*a, *b, *c, *d]))),
            _ => Err("malformed INTEGER element"),
        },
        20 => match bytes {
            [a, b, c, d, e, f, g, h] => Ok(i64::from_be_bytes([*a, *b, *c, *d, *e, *f, *g, *h])),
            _ => Err("malformed BIGINT element"),
        },
        // An OID column is an unsigned 32-bit integer.
        26 => match bytes {
            [a, b, c, d] => Ok(i64::from(u32::from_be_bytes([*a, *b, *c, *d]))),
            _ => Err("malformed OID element"),
        },
        _ => Err("this element is not an integer"),
    }
}

fn array_float(element: u32, bytes: &[u8]) -> Result<f64, &'static str> {
    match element {
        700 => match bytes {
            [a, b, c, d] => Ok(f64::from(f32::from_be_bytes([*a, *b, *c, *d]))),
            _ => Err("malformed REAL element"),
        },
        701 => match bytes {
            [a, b, c, d, e, f, g, h] => Ok(f64::from_be_bytes([*a, *b, *c, *d, *e, *f, *g, *h])),
            _ => Err("malformed DOUBLE PRECISION element"),
        },
        _ => Err("this element is not a floating-point number"),
    }
}

fn array_micros(bytes: &[u8]) -> Result<i64, &'static str> {
    match bytes {
        [a, b, c, d, e, f, g, h] => Ok(i64::from_be_bytes([*a, *b, *c, *d, *e, *f, *g, *h])),
        _ => Err("malformed timestamp element"),
    }
}

impl ArrayElement for bool {
    fn decode_element(element: u32, bytes: &[u8]) -> Result<Self, &'static str> {
        match (element, bytes) {
            (16, [value]) => Ok(*value != 0),
            _ => Err("this element is not a boolean"),
        }
    }

    fn decode_text(text: &str) -> Result<Self, &'static str> {
        match text.trim() {
            "t" | "true" | "TRUE" => Ok(true),
            "f" | "false" | "FALSE" => Ok(false),
            _ => Err("an array element is not a boolean"),
        }
    }
}

macro_rules! array_integer_element {
    ($($ty:ty),* $(,)?) => {
        $(
            impl ArrayElement for $ty {
                fn decode_element(element: u32, bytes: &[u8]) -> Result<Self, &'static str> {
                    let value = array_integer(element, bytes)?;
                    <$ty>::try_from(value)
                        .map_err(|_| "an array element is out of range for its Rust type")
                }

                fn decode_text(text: &str) -> Result<Self, &'static str> {
                    text.trim()
                        .parse()
                        .map_err(|_| "an array element is not a valid integer")
                }
            }
        )*
    };
}

array_integer_element!(i8, i16, i32, i64, u16, u32, u64);

/// A 128-bit integer arrives as a `NUMERIC`, because no PostgreSQL integer is that wide.
macro_rules! array_wide_integer_element {
    ($($ty:ty),* $(,)?) => {
        $(
            impl ArrayElement for $ty {
                fn decode_element(element: u32, bytes: &[u8]) -> Result<Self, &'static str> {
                    if element != NUMERIC_OID {
                        return Err("this element is not a NUMERIC");
                    }
                    let (negative, value) = decode_numeric_binary(bytes)?;
                    if negative {
                        return Err("a negative NUMERIC does not fit in an unsigned integer");
                    }
                    <$ty>::try_from(value)
                        .map_err(|_| "an array element is out of range for its Rust type")
                }

                fn decode_text(text: &str) -> Result<Self, &'static str> {
                    text.trim()
                        .parse()
                        .map_err(|_| "an array element is not a valid integer")
                }
            }
        )*
    };
}

array_wide_integer_element!(u128);

impl ArrayElement for f32 {
    fn decode_element(element: u32, bytes: &[u8]) -> Result<Self, &'static str> {
        Ok(array_float(element, bytes)? as f32)
    }

    fn decode_text(text: &str) -> Result<Self, &'static str> {
        text.trim()
            .parse()
            .map_err(|_| "an array element is not a number")
    }
}

impl ArrayElement for f64 {
    fn decode_element(element: u32, bytes: &[u8]) -> Result<Self, &'static str> {
        array_float(element, bytes)
    }

    fn decode_text(text: &str) -> Result<Self, &'static str> {
        text.trim()
            .parse()
            .map_err(|_| "an array element is not a number")
    }
}

impl ArrayElement for String {
    fn decode_element(element: u32, bytes: &[u8]) -> Result<Self, &'static str> {
        match element {
            19 | 25 | 1042 | 1043 => core::str::from_utf8(bytes)
                .map(str::to_owned)
                .map_err(|_| "a text array element is not valid UTF-8"),
            // A decimal's Rust type is also `String`, so the element's OID decides which of
            // the two a `Vec<String>` holds. One impl therefore serves both.
            NUMERIC_OID => decode_numeric_decimal(bytes),
            _ => Err("this element is not text"),
        }
    }

    fn decode_text(text: &str) -> Result<Self, &'static str> {
        Ok(text.to_owned())
    }
}

impl ArrayElement for uuid::Uuid {
    fn decode_element(element: u32, bytes: &[u8]) -> Result<Self, &'static str> {
        if element != 2950 {
            return Err("this element is not a UUID");
        }
        let bytes: [u8; 16] = bytes.try_into().map_err(|_| "malformed UUID element")?;
        Ok(uuid::Uuid::from_bytes(bytes))
    }

    fn decode_text(text: &str) -> Result<Self, &'static str> {
        text.trim()
            .parse()
            .map_err(|_| "an array element is not a UUID")
    }
}

impl ArrayElement for chrono::NaiveDate {
    fn decode_element(element: u32, bytes: &[u8]) -> Result<Self, &'static str> {
        if element != 1082 {
            return Err("this element is not a DATE");
        }
        let days = array_integer(23, bytes)?;
        postgres_epoch()
            .checked_add_signed(chrono::Duration::days(days))
            .ok_or("the date is out of range")
    }

    fn decode_text(text: &str) -> Result<Self, &'static str> {
        chrono::NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d")
            .map_err(|_| "an array element is not a date")
    }
}

impl ArrayElement for chrono::NaiveTime {
    fn decode_element(element: u32, bytes: &[u8]) -> Result<Self, &'static str> {
        if element != 1083 {
            return Err("this element is not a TIME");
        }
        let micros = array_micros(bytes)?;
        let micros = u32::try_from(micros).map_err(|_| "the time is out of range")?;
        chrono::NaiveTime::from_num_seconds_from_midnight_opt(
            micros / 1_000_000,
            (micros % 1_000_000) * 1_000,
        )
        .ok_or("the time is out of range")
    }

    fn decode_text(text: &str) -> Result<Self, &'static str> {
        chrono::NaiveTime::parse_from_str(text.trim(), "%H:%M:%S%.f")
            .map_err(|_| "an array element is not a time")
    }
}

impl ArrayElement for chrono::NaiveDateTime {
    fn decode_element(element: u32, bytes: &[u8]) -> Result<Self, &'static str> {
        if element != 1114 && element != 1184 {
            return Err("this element is not a TIMESTAMP");
        }
        let micros = array_micros(bytes)?;
        postgres_epoch()
            .and_time(chrono::NaiveTime::MIN)
            .checked_add_signed(chrono::Duration::microseconds(micros))
            .ok_or("the timestamp is out of range")
    }

    fn decode_text(text: &str) -> Result<Self, &'static str> {
        chrono::NaiveDateTime::parse_from_str(text.trim(), "%Y-%m-%d %H:%M:%S%.f")
            .map_err(|_| "an array element is not a timestamp")
    }
}

impl ArrayElement for chrono::DateTime<chrono::Utc> {
    fn decode_element(element: u32, bytes: &[u8]) -> Result<Self, &'static str> {
        let naive = chrono::NaiveDateTime::decode_element(element, bytes)?;
        Ok(naive.and_utc())
    }

    fn decode_text(text: &str) -> Result<Self, &'static str> {
        // PostgreSQL prints a timestamp with a two-digit offset (`+00`) or a colon-separated
        // one (`+00:00`), while chrono's `%:z` wants the colon and `%z` wants no colon at
        // all. Normalising the offset once covers both spellings.
        let text = text.trim();
        let offset = text
            .rfind(['+', '-'])
            .filter(|index| *index > 10)
            .map(|index| (&text[..index], &text[index..]));
        let normalised = match offset {
            Some((head, offset)) => {
                let digits = offset.trim_start_matches(['+', '-']);
                if digits.len() == 2 && digits.bytes().all(|byte| byte.is_ascii_digit()) {
                    Some(format!("{head}{offset}:00"))
                } else if digits.len() == 4 && digits.bytes().all(|byte| byte.is_ascii_digit()) {
                    Some(format!("{head}{}:{}", &offset[..3], &offset[3..]))
                } else {
                    None
                }
            }
            None => None,
        };
        let text = normalised.as_deref().unwrap_or(text);
        chrono::DateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f%:z")
            .or_else(|_| chrono::DateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f%:z"))
            .map(|value| value.with_timezone(&chrono::Utc))
            .map_err(|_| "an array element is not a timestamp with a time zone")
    }
}

impl ArrayElement for serde_json::Value {
    fn decode_element(element: u32, bytes: &[u8]) -> Result<Self, &'static str> {
        let text = match element {
            114 => bytes,
            // The binary JSONB form is a one-byte version followed by the document.
            3802 => match bytes.split_first() {
                Some((1, rest)) => rest,
                _ => return Err("malformed JSONB element"),
            },
            _ => return Err("this element is not JSON"),
        };
        serde_json::from_slice(text).map_err(|_| "a JSON array element is not valid JSON")
    }

    fn decode_text(text: &str) -> Result<Self, &'static str> {
        serde_json::from_str(text).map_err(|_| "an array element is not valid JSON")
    }
}

impl ArrayElement for SqlInterval {
    fn decode_element(element: u32, bytes: &[u8]) -> Result<Self, &'static str> {
        if element != INTERVAL_OID {
            return Err("this element is not an INTERVAL");
        }
        decode_interval_binary(bytes)
    }

    fn decode_text(text: &str) -> Result<Self, &'static str> {
        text.trim()
            .parse()
            .map_err(|_| "an array element is not an interval")
    }
}

/// Read one network element, which is the same five fields as the scalar format.
fn array_network(bytes: &[u8]) -> Result<(core::net::IpAddr, u8), &'static str> {
    let header = bytes.get(..4).ok_or("malformed network element")?;
    let prefix = header[1];
    match (header[0], &bytes[4..]) {
        (2, [a, b, c, d]) => Ok((
            core::net::IpAddr::V4(core::net::Ipv4Addr::new(*a, *b, *c, *d)),
            prefix,
        )),
        (3, bytes) => {
            let bytes: [u8; 16] = bytes.try_into().map_err(|_| "malformed network element")?;
            Ok((
                core::net::IpAddr::V6(core::net::Ipv6Addr::from(bytes)),
                prefix,
            ))
        }
        _ => Err("malformed network element"),
    }
}

impl ArrayElement for Inet {
    fn decode_element(element: u32, bytes: &[u8]) -> Result<Self, &'static str> {
        if element != INET_OID {
            return Err("this element is not an INET");
        }
        let (address, prefix) = array_network(bytes)?;
        Self::new(&format!("{address}/{prefix}")).map_err(|_| "an array element is not an address")
    }

    fn decode_text(text: &str) -> Result<Self, &'static str> {
        Self::new(text.trim()).map_err(|_| "an array element is not an address")
    }
}

impl ArrayElement for Cidr {
    fn decode_element(element: u32, bytes: &[u8]) -> Result<Self, &'static str> {
        if element != CIDR_OID {
            return Err("this element is not a CIDR");
        }
        let (address, prefix) = array_network(bytes)?;
        Self::new(&format!("{address}/{prefix}")).map_err(|_| "an array element is not a network")
    }

    fn decode_text(text: &str) -> Result<Self, &'static str> {
        Self::new(text.trim()).map_err(|_| "an array element is not a network")
    }
}

impl ArrayElement for BitString {
    fn decode_element(element: u32, bytes: &[u8]) -> Result<Self, &'static str> {
        if element != BIT_OID && element != VARBIT_OID {
            return Err("this element is not a bit string");
        }
        let header = bytes.get(..4).ok_or("malformed bit element")?;
        let length = i32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let length = usize::try_from(length).map_err(|_| "the bit length is negative")?;
        Self::from_bytes(&bytes[4..], length).map_err(|_| "an array element is not a bit string")
    }

    fn decode_text(text: &str) -> Result<Self, &'static str> {
        Self::new(text.trim()).map_err(|_| "an array element is not a bit string")
    }
}

macro_rules! impl_decode_array {
    ($($ty:ty),* $(,)?) => {
        $(
            impl Decode<Sqlx> for Vec<$ty> {
                fn decode(row: &PgRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
                    decode_array(row, offset)
                }
            }
        )*
    };
}

impl_decode_array!(
    bool,
    i8,
    i16,
    i32,
    i64,
    u16,
    u32,
    u64,
    f32,
    f64,
    String,
    uuid::Uuid,
    chrono::NaiveDate,
    chrono::NaiveTime,
    chrono::NaiveDateTime,
    chrono::DateTime<chrono::Utc>,
    serde_json::Value,
    u128,
    SqlInterval,
    Inet,
    Cidr,
    BitString,
    Range<i32>,
    Range<i64>,
    Range<String>,
    Range<chrono::NaiveDateTime>,
    Range<chrono::DateTime<chrono::Utc>>,
    Range<chrono::NaiveDate>,
);

/// Parse PostgreSQL's text array literal: `{a,"b,c",NULL}`.
///
/// A NULL element is an error because a `Vec<T>` cannot hold one, and a nested brace is an
/// error because only one dimension decodes into a `Vec`.
fn decode_text_array<T: ArrayElement>(text: &str) -> Result<Vec<T>, &'static str> {
    let inner = text
        .trim()
        .strip_prefix('{')
        .and_then(|text| text.strip_suffix('}'))
        .ok_or("malformed array value")?;
    if inner.trim().is_empty() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    let mut element = String::new();
    let mut quoted = false;
    let mut escaped = false;
    let mut quoted_any = false;
    for ch in inner.chars() {
        if escaped {
            element.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' if quoted => escaped = true,
            '"' => {
                quoted = !quoted;
                quoted_any = true;
            }
            '{' | '}' if !quoted => {
                return Err("only one-dimensional arrays decode into a `Vec`");
            }
            ',' if !quoted => {
                out.push(text_element::<T>(&element, quoted_any)?);
                element.clear();
                quoted_any = false;
            }
            other => element.push(other),
        }
    }
    if quoted {
        return Err("malformed array value");
    }
    out.push(text_element::<T>(&element, quoted_any)?);
    Ok(out)
}

/// One text element, where an unquoted `NULL` is the SQL `NULL`.
fn text_element<T: ArrayElement>(text: &str, was_quoted: bool) -> Result<T, &'static str> {
    if !was_quoted && text == "NULL" {
        return Err("the array contains a NULL element, which a `Vec<T>` cannot hold");
    }
    T::decode_text(text)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use besu_core::{Bound, TypeDef};

    /// Encode a range exactly as the driver sends it.
    fn encode(element: &TypeDef, range: Range<Value>) -> Vec<u8> {
        let mut out = Vec::new();
        write_range(element, &range, &mut out).unwrap();
        out
    }

    #[test]
    fn a_range_is_written_with_its_flags_and_read_back() {
        // The empty range is one flag byte and nothing else.
        assert_eq!(encode(&TypeDef::Int4, Range::empty()), [0x01]);
        assert_eq!(
            decode_range_binary::<i32>(&encode(&TypeDef::Int4, Range::empty())),
            Ok(Range::empty())
        );

        // `[1,5]` is both bounds finite and included: 0x02 | 0x04.
        let bytes = encode(
            &TypeDef::Int4,
            Range::new(
                Bound::Included(Value::Int(1)),
                Bound::Included(Value::Int(5)),
            ),
        );
        assert_eq!(
            bytes,
            [
                0x06, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x04, 0x00,
                0x00, 0x00, 0x05,
            ]
        );
        assert_eq!(decode_range_binary::<i32>(&bytes), Ok(Range::closed(1, 5)));

        // `(1,5]`: the lower bound is excluded, so only 0x04 is set.
        let bytes = encode(
            &TypeDef::Int4,
            Range::new(
                Bound::Excluded(Value::Int(1)),
                Bound::Included(Value::Int(5)),
            ),
        );
        assert_eq!(bytes[0], 0x04);
        assert_eq!(
            decode_range_binary::<i32>(&bytes),
            Ok(Range::open_closed(1, 5))
        );

        // `(,5)`: an infinite lower bound sets 0x08, and only the upper bound is written.
        let bytes = encode(
            &TypeDef::Int4,
            Range::new(Bound::Unbounded, Bound::Excluded(Value::Int(5))),
        );
        assert_eq!(bytes[0], 0x08);
        assert_eq!(bytes.len(), 1 + 4 + 4);
        assert_eq!(
            decode_range_binary::<i32>(&bytes),
            Ok(Range::new(Bound::Unbounded, Bound::Excluded(5)))
        );

        // `(,)`: both ends are infinite.
        let bytes = encode(
            &TypeDef::Int4,
            Range::new(Bound::<Value>::Unbounded, Bound::Unbounded),
        );
        assert_eq!(bytes, [0x18]);
        assert_eq!(
            decode_range_binary::<i32>(&bytes),
            Ok(Range::new(Bound::Unbounded, Bound::Unbounded))
        );
    }

    #[test]
    fn an_empty_range_is_written_as_the_empty_flag_even_with_bounds() {
        // `Range::empty()` has no bounds, so the flags carry only the empty marker.
        assert_eq!(encode(&TypeDef::Int8, Range::empty()), [0x01]);
    }

    #[test]
    fn a_range_of_timestamps_and_a_numeric_range_round_trip() {
        let start = chrono::NaiveDate::from_ymd_opt(2020, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let end = chrono::NaiveDate::from_ymd_opt(2021, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let bytes = encode(
            &TypeDef::Timestamp,
            Range::new(
                Bound::Included(Value::Timestamp(start)),
                Bound::Excluded(Value::Timestamp(end)),
            ),
        );
        assert_eq!(
            decode_range_binary::<chrono::NaiveDateTime>(&bytes),
            Ok(Range::closed_open(start, end))
        );

        let bytes = encode(
            &TypeDef::Decimal(0, 0),
            Range::new(
                Bound::Included(Value::Decimal("1.25".into())),
                Bound::Excluded(Value::Decimal("9".into())),
            ),
        );
        // The decimal bounds are NUMERIC, so the subtype's payload is a `numeric` body.
        assert_eq!(
            decode_range_binary::<String>(&bytes),
            Ok(Range::closed_open("1.25".to_owned(), "9".to_owned()))
        );
    }

    #[test]
    fn a_range_whose_value_does_not_match_its_subtype_is_an_error() {
        let mut out = Vec::new();
        assert!(
            write_range(
                &TypeDef::Int4,
                &Range::new(
                    Bound::Included(Value::Text("one".into())),
                    Bound::Excluded(Value::Int(5)),
                ),
                &mut out,
            )
            .is_err()
        );
    }

    #[test]
    fn a_malformed_range_does_not_panic() {
        assert!(decode_range_binary::<i32>(&[]).is_err());
        assert!(decode_range_binary::<i32>(&[0x00]).is_err());
        assert!(decode_range_binary::<i32>(&[0x02, 0x00, 0x00]).is_err());
        // `LB_NULL` cannot be represented by a Rust range.
        assert!(decode_range_binary::<i32>(&[0x20]).is_err());
    }

    #[test]
    fn every_builtin_range_has_its_oid() {
        assert_eq!(
            element_oid(&TypeDef::Range(TypeDef::Int4.into())),
            Some(3904)
        );
        assert_eq!(
            element_oid(&TypeDef::Range(TypeDef::Int8.into())),
            Some(3926)
        );
        assert_eq!(
            element_oid(&TypeDef::Range(TypeDef::Decimal(10, 2).into())),
            Some(3906)
        );
        assert_eq!(
            element_oid(&TypeDef::Range(TypeDef::Timestamp.into())),
            Some(3908)
        );
        assert_eq!(
            element_oid(&TypeDef::Range(TypeDef::TimestampTz.into())),
            Some(3910)
        );
        assert_eq!(
            element_oid(&TypeDef::Range(TypeDef::Date.into())),
            Some(3912)
        );
        // A `TEXT` range has no PostgreSQL type, and an extension type's OID is not stable.
        assert_eq!(element_oid(&TypeDef::Range(TypeDef::Text.into())), None);
        assert_eq!(
            element_oid(&TypeDef::Geometry {
                kind: besu_core::GeometryKind::Point,
                srid: None,
            }),
            None
        );
    }
}
