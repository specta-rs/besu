use besu_core::{Decode, Driver, Error, Sql, TransactionDriver, Value};
use sqlx::{
    Arguments, PgPool, Postgres as SqlxPostgres, Row, ValueRef,
    postgres::{
        PgArgumentBuffer, PgArguments, PgQueryResult, PgRow, PgTypeInfo, PgValueFormat, PgValueRef,
    },
};

use crate::Postgres;

/// The OID of PostgreSQL's built-in `NUMERIC` type, which is stable across servers.
const NUMERIC_OID: u32 = 1700;

/// The OID of PostgreSQL's built-in `REAL` (`FLOAT4`) type.
const FLOAT4_OID: u32 = 700;

/// Whether a value's declared SQL type is the built-in type with this OID.
fn type_is(value: &PgValueRef<'_>, oid: u32) -> bool {
    value
        .type_info()
        .type_eq(&PgTypeInfo::with_oid(sqlx::postgres::types::Oid(oid)))
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
        Value::Bytes(v) => LENGTH_PREFIX + v.len(),
        Value::Uuid(_) => LENGTH_PREFIX + 16,
        Value::Date(_) => LENGTH_PREFIX + 4,
        Value::Time(_) => LENGTH_PREFIX + 8,
        Value::Timestamp(_) | Value::TimestampTz(_) => LENGTH_PREFIX + 8,
        // The JSONB form is a one-byte version plus the document.
        Value::Json(v) => LENGTH_PREFIX + 1 + v.len(),
        // `Value` is `#[non_exhaustive]`. An unknown variant fails to bind before this
        // estimate matters, so reserving nothing for it is harmless.
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
            Value::Bytes(v) => add(&mut arguments, v.as_slice())?,
            Value::Uuid(v) => add(&mut arguments, *v)?,
            Value::Date(v) => add(&mut arguments, *v)?,
            Value::Time(v) => add(&mut arguments, *v)?,
            Value::Timestamp(v) => add(&mut arguments, *v)?,
            Value::TimestampTz(v) => add(&mut arguments, *v)?,
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
    row.try_get(index)
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
    String,
    Vec<u8>,
    uuid::Uuid,
    chrono::NaiveDate,
    chrono::NaiveTime,
    chrono::NaiveDateTime,
    chrono::DateTime<chrono::Utc>,
);

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
    if usize::from(ndigits) != (bytes.len() - 8) / 2 {
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
        let value: sqlx::types::Json<serde_json::Value> = decode_via_sqlx(row, offset)?;
        Ok(value.0)
    }
}
