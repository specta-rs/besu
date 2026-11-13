use besu_core::{Decode, Driver, Error, Sql, TransactionDriver, Value};
use core::fmt::Write as _;
use num_traits::ToPrimitive;
use sqlx::{
    Arguments, Column as _, MySql, MySqlPool, Row, Type, ValueRef,
    mysql::{MySqlArguments, MySqlConnectOptions, MySqlPoolOptions, MySqlQueryResult, MySqlRow},
    types::BigDecimal,
};

use crate::MySql as MySqlDialect;

/// A [`besu_core::Driver`] for MySQL, backed by [`sqlx`].
///
/// The pool is created from a connection URL. [`Sqlx::new`] pins the session to UTF-8
/// (`utf8mb4`) and UTC so that text and timestamp values mean the same thing on every
/// connection; [`Sqlx::connect`] uses the URL exactly as written.
#[derive(Debug, Clone)]
pub struct Sqlx {
    pool: MySqlPool,
}

impl Sqlx {
    /// Connect to a MySQL server using a URL such as
    /// `mysql://user:password@localhost:3306/app`.
    ///
    /// The connection is pinned to `utf8mb4` and `+00:00`, because [`chrono`] timestamps are
    /// encoded as UTC and a non-UTF-8 connection would mangle text.
    pub async fn new(url: &str) -> Result<Self, sqlx::Error> {
        let options = url
            .parse::<MySqlConnectOptions>()?
            .charset("utf8mb4")
            .timezone("+00:00".to_owned());
        Self::with_options(options).await
    }

    /// Connect to a MySQL server, leaving every option exactly as the URL spells it.
    pub async fn connect(url: &str) -> Result<Self, sqlx::Error> {
        Self::with_options(url.parse()?).await
    }

    async fn with_options(options: MySqlConnectOptions) -> Result<Self, sqlx::Error> {
        Ok(Self {
            pool: MySqlPoolOptions::new().connect_with(options).await?,
        })
    }

    /// Wrap an existing pool.
    pub fn from_pool(pool: MySqlPool) -> Self {
        Self { pool }
    }

    /// The underlying pool, for anything Besu does not cover.
    pub fn pool(&self) -> &MySqlPool {
        &self.pool
    }
}

fn add<'q, T>(arguments: &mut MySqlArguments, value: T) -> Result<(), sqlx::Error>
where
    T: 'q + sqlx::Encode<'q, MySql> + sqlx::Type<MySql>,
{
    arguments.add(value).map_err(sqlx::Error::Encode)
}

/// A 128-bit integer, encoded as the decimal text MySQL stores in a `DECIMAL(39,0)`.
///
/// `sqlx` encodes a [`BigDecimal`] by formatting it to a `String` and then writing that as
/// length-encoded text, so a `BigDecimal` built for one parameter allocates twice: once for
/// the digit vector inside its `BigInt` and once for the string. Decimal text is what goes
/// over the wire either way, so formatting the value directly produces the same bytes with
/// no allocation at all. The declared type stays `NEWDECIMAL`, so a cached prepared statement
/// sees exactly the parameter type it saw before.
#[derive(Clone, Copy)]
struct Decimal {
    negative: bool,
    magnitude: u128,
}

impl Decimal {
    fn from_u128(value: u128) -> Self {
        Self {
            negative: false,
            magnitude: value,
        }
    }

    fn from_i128(value: i128) -> Self {
        Self {
            negative: value < 0,
            magnitude: value.unsigned_abs(),
        }
    }
}

impl Type<MySql> for Decimal {
    fn type_info() -> sqlx::mysql::MySqlTypeInfo {
        <BigDecimal as Type<MySql>>::type_info()
    }

    fn compatible(ty: &sqlx::mysql::MySqlTypeInfo) -> bool {
        <BigDecimal as Type<MySql>>::compatible(ty)
    }
}

impl sqlx::Encode<'_, MySql> for Decimal {
    fn encode_by_ref(
        &self,
        buf: &mut Vec<u8>,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        // A `u128` is at most 39 digits, and an `i128` is one sign longer, so the text always
        // fits in 40 bytes. That is under 251, which means its length always fits in the
        // one-byte form of MySQL's length-encoded integer — the same encoding sqlx uses for
        // a `BigDecimal`.
        let mut text = DecimalText {
            bytes: [0; 40],
            len: 0,
        };
        if self.negative {
            text.write_str("-")?;
        }
        // `Display` for `u128` divides by `10^19` and then formats the halves, which is a
        // good deal cheaper than peeling one digit off at a time.
        write!(&mut text, "{}", self.magnitude)?;

        buf.push(text.len as u8);
        buf.extend_from_slice(&text.bytes[..text.len]);

        Ok(sqlx::encode::IsNull::No)
    }
}

/// A fixed-size buffer that [`write!`] can format into without allocating.
struct DecimalText {
    bytes: [u8; 40],
    len: usize,
}

impl core::fmt::Write for DecimalText {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let end = self.len + text.len();
        let destination = self.bytes.get_mut(self.len..end).ok_or(core::fmt::Error)?;
        destination.copy_from_slice(text.as_bytes());
        self.len = end;
        Ok(())
    }
}

fn arguments(sql: &Sql<MySqlDialect>) -> Result<MySqlArguments, sqlx::Error> {
    let values = sql.args();
    let mut arguments = MySqlArguments::default();

    // The parameter count is known before anything is encoded, so asking for the buffers up
    // front saves the value, type and null buffers each growing a handful of times (and
    // copying themselves) on the way to the final size. Eight bytes is a rough size for a
    // small parameter; a 128-bit value is decimal text and always at most 40 digits plus its
    // one-byte length. Anything larger still grows once more rather than being pessimised.
    let encoded_size: usize = values
        .iter()
        .map(|value| match value {
            Value::UInt128(_) | Value::Int128(_) => 41,
            Value::Decimal(v) => v.len() + 9,
            _ => 8,
        })
        .sum();
    arguments.reserve(values.len(), encoded_size);

    for value in values {
        match value {
            Value::Null => add(&mut arguments, None::<i64>)?,
            Value::Bool(v) => add(&mut arguments, *v)?,
            Value::Int(v) => add(&mut arguments, *v)?,
            // MySQL has real unsigned integers, so the full `u64` range round-trips.
            Value::UInt(v) => add(&mut arguments, *v)?,
            // A `u128` column is `DECIMAL(39,0)`, so a 128-bit value goes over the wire as
            // decimal text.
            Value::UInt128(v) => add(&mut arguments, Decimal::from_u128(*v))?,
            Value::Int128(v) => add(&mut arguments, Decimal::from_i128(*v))?,
            Value::Float(v) => add(&mut arguments, *v)?,
            Value::Text(v) => add(&mut arguments, v.as_str())?,
            // MySQL coerces a text parameter to the column's type, so a label binds exactly
            // like text.
            Value::EnumLabel(v) => add(&mut arguments, v.as_str())?,
            // MySQL coerces a string parameter to the column's type, so the exact decimal
            // text goes over the wire unchanged and the server rounds it to the column's
            // scale. Building a `BigDecimal` would copy the digits twice for no gain.
            Value::Decimal(v) => add(&mut arguments, v.as_str())?,
            Value::Bytes(v) => add(&mut arguments, v.as_slice())?,
            Value::Uuid(v) => add(&mut arguments, *v)?,
            Value::Date(v) => add(&mut arguments, *v)?,
            Value::Time(v) => add(&mut arguments, *v)?,
            Value::Timestamp(v) => add(&mut arguments, *v)?,
            Value::TimestampTz(v) => add(&mut arguments, *v)?,
            Value::Json(v) => {
                let text = std::str::from_utf8(v).unwrap_or("null");
                add(&mut arguments, if text.is_empty() { "null" } else { text })?
            }
            // MySQL has no array type. Binding `NULL` instead would turn a programming
            // mistake into a missing value, so the statement fails before it is sent.
            Value::Array { .. } => {
                return Err(<Sqlx as Driver>::error(Error::encode(
                    value.display(),
                    "`mysql` has no array type; store the elements as JSON instead",
                )));
            }
            _ => add(&mut arguments, None::<i64>)?,
        }
    }
    Ok(arguments)
}

/// Encode a statement's parameters exactly as the driver would, for the benchmarks.
///
/// This is hidden from the documentation and is not part of the supported API. The `encode`
/// benchmark has to time [`arguments`] with no database in the way, and that function is
/// private to this crate, so this is the one seam it gets.
#[doc(hidden)]
pub fn arguments_for_bench(sql: &Sql<MySqlDialect>) -> Result<MySqlArguments, sqlx::Error> {
    arguments(sql)
}

impl Driver for Sqlx {
    type Dialect = MySqlDialect;
    type Error = sqlx::Error;
    type Row = MySqlRow;
    type Output = MySqlQueryResult;

    fn error(err: Error) -> Self::Error {
        sqlx::Error::Protocol(err.to_string())
    }

    fn row_len(row: &Self::Row) -> usize {
        row.len()
    }

    fn is_null(row: &Self::Row, index: usize) -> bool {
        row.try_get_raw(index)
            .map(|value| sqlx::ValueRef::is_null(&value))
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
        let mut offset = index;
        <String as Decode<Self>>::decode(row, &mut offset)
    }

    async fn query(&self, sql: &Sql<MySqlDialect>) -> Result<Vec<Self::Row>, Self::Error> {
        let arguments = arguments(sql)?;
        sqlx::query_with(sql.sql(), arguments)
            .fetch_all(&self.pool)
            .await
    }

    async fn execute(&self, sql: &Sql<MySqlDialect>) -> Result<Self::Output, Self::Error> {
        let arguments = arguments(sql)?;
        sqlx::query_with(sql.sql(), arguments)
            .execute(&self.pool)
            .await
    }

    async fn query_stream<'a>(
        &'a self,
        sql: &'a Sql<MySqlDialect>,
    ) -> Result<besu_core::RowStream<'a, Self>, Self::Error> {
        let arguments = arguments(sql)?;
        Ok(sqlx::query_with(sql.sql(), arguments).fetch(&self.pool))
    }
}

impl besu::SchemaDriver for Sqlx {
    type Ddl = crate::MySqlDdl;

    async fn introspect(&self) -> Result<besu::schema::Schema, Self::Error> {
        let db = besu::Database::new(self.clone());
        crate::introspect::introspect(&db).await
    }
}

impl TransactionDriver for Sqlx {
    type Transaction<'a> = sqlx::Transaction<'a, MySql>;

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
        sql: &Sql<MySqlDialect>,
    ) -> Result<Vec<Self::Row>, Self::Error> {
        let arguments = arguments(sql)?;
        sqlx::query_with(sql.sql(), arguments)
            .fetch_all(&mut **tx)
            .await
    }

    async fn txn_execute<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &Sql<MySqlDialect>,
    ) -> Result<Self::Output, Self::Error> {
        let arguments = arguments(sql)?;
        sqlx::query_with(sql.sql(), arguments)
            .execute(&mut **tx)
            .await
    }

    async fn txn_query_stream<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &'c Sql<MySqlDialect>,
    ) -> Result<besu_core::RowStream<'c, Self>, Self::Error> {
        let arguments = arguments(sql)?;
        Ok(sqlx::query_with(sql.sql(), arguments).fetch(&mut **tx))
    }
}

// ---------------------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------------------

fn decode_via_sqlx<T>(row: &MySqlRow, offset: &mut usize) -> Result<T, sqlx::Error>
where
    T: for<'r> sqlx::Decode<'r, MySql> + sqlx::Type<MySql>,
{
    let index = *offset;
    *offset += 1;
    row.try_get(index)
}

/// Decode an already-fetched value, reporting a failure the way [`Row::try_get`] does.
///
/// The type has already been checked, so this is the second half of `try_get` without a
/// second trip through the row to fetch the value and its type information again.
fn decode_checked<'r, T>(
    value: sqlx::mysql::MySqlValueRef<'r>,
    index: usize,
) -> Result<T, sqlx::Error>
where
    T: sqlx::Decode<'r, MySql>,
{
    T::decode(value).map_err(|source| sqlx::Error::ColumnDecode {
        index: index.to_string(),
        source,
    })
}

macro_rules! impl_decode {
    ($($ty:ty),* $(,)?) => {
        $(
            impl Decode<Sqlx> for $ty {
                fn decode(row: &MySqlRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
                    decode_via_sqlx(row, offset)
                }
            }
        )*
    };
}

impl_decode!(
    bool,
    f64,
    Vec<u8>,
    uuid::Uuid,
    chrono::NaiveDate,
    chrono::NaiveTime,
    chrono::NaiveDateTime,
    chrono::DateTime<chrono::Utc>,
);

impl Decode<Sqlx> for String {
    fn decode(row: &MySqlRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;

        // A `DECIMAL` column arrives as text and `sqlx` refuses to hand it to `String`
        // because the declared type is not a string type. Reading those bytes directly keeps
        // the column's own scale — 0.00 stays `0.00` — which `BigDecimal` normalises away.
        if <BigDecimal as Type<MySql>>::compatible(row.try_get_raw(index)?.type_info().as_ref()) {
            let value = row.try_get_raw(index)?;
            let text = <&str as sqlx::Decode<MySql>>::decode(value)
                .map_err(|err| <Sqlx as Driver>::error(Error::decode(index, err.to_string())))?;
            *offset += 1;
            return Ok(text.to_owned());
        }

        // Most text columns decode directly, but MySQL flags the `information_schema`
        // text columns as binary. Those still hold UTF-8, so fall back to reading the
        // bytes and validating them rather than rejecting a perfectly good string.
        if let Ok(value) = row.try_get::<String, _>(index) {
            *offset += 1;
            return Ok(value);
        }

        let bytes = decode_via_sqlx::<Vec<u8>>(row, offset)?;
        String::from_utf8(bytes).map_err(|_| {
            <Sqlx as Driver>::error(Error::decode(index, "the value is not valid UTF-8"))
        })
    }
}

/// Decode an integer column as `i64`, whichever signedness MySQL reports for it.
///
/// Most columns are signed, but MySQL marks a few computed values — `COUNT(*)` for
/// instance — as `BIGINT UNSIGNED`, and `sqlx` refuses to decode an unsigned column
/// through a signed type. The type information says which one the column is, so asking it
/// first avoids trying `i64`, building (and discarding) a `ColumnDecode` error, and then
/// trying `u64`: an unsigned column would otherwise allocate a whole error message for
/// every value.
fn decode_integer(row: &MySqlRow, offset: &mut usize) -> Result<i64, sqlx::Error> {
    let index = *offset;
    let value = row.try_get_raw(index)?;
    let type_info = value.type_info().into_owned();

    if <i64 as Type<MySql>>::compatible(&type_info) {
        *offset += 1;
        return decode_checked(value, index);
    }

    if <u64 as Type<MySql>>::compatible(&type_info) {
        *offset += 1;
        let value: u64 = decode_checked(value, index)?;
        return i64::try_from(value).map_err(|_| {
            <Sqlx as Driver>::error(Error::decode(
                index,
                "a MySQL unsigned integer is larger than i64::MAX",
            ))
        });
    }

    // Neither type matches; decode once more so the caller gets sqlx's own explanation.
    decode_via_sqlx::<i64>(row, offset)
}

/// MySQL narrows integers on the wire: a `BIGINT` and a `TINYINT` both arrive as a
/// little-endian buffer of one to eight bytes. Decoding through `i64` and range-checking
/// keeps a wrong-width value an error rather than a silent truncation.
macro_rules! impl_decode_int {
    ($($ty:ty),* $(,)?) => {
        $(
            impl Decode<Sqlx> for $ty {
                fn decode(row: &MySqlRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
                    let index = *offset;
                    let value = decode_integer(row, offset)?;
                    <$ty>::try_from(value).map_err(|_| {
                        <Sqlx as Driver>::error(Error::decode(
                            index,
                            concat!("a MySQL integer does not fit in ", stringify!($ty)),
                        ))
                    })
                }
            }
        )*
    };
}

impl_decode_int!(i8, i16, i32, i64, u8, u16, u32);

impl Decode<Sqlx> for f32 {
    fn decode(row: &MySqlRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        // MySQL sends `FLOAT` as either 4 or 8 bytes; going through `f64` handles both.
        let value: f64 = decode_via_sqlx(row, offset)?;
        Ok(value as f32)
    }
}

impl Decode<Sqlx> for u64 {
    fn decode(row: &MySqlRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;

        // An `UNSIGNED BIGINT` column only decodes as `u64`, while a signed one only
        // decodes as `i64`. The type information knows which, so ask it rather than trying
        // `u64` and throwing away an error for every signed column.
        let value = row.try_get_raw(index)?;
        if <u64 as Type<MySql>>::compatible(value.type_info().as_ref()) {
            *offset += 1;
            return decode_checked(value, index);
        }

        let value = decode_integer(row, offset)?;
        u64::try_from(value).map_err(|_| {
            <Sqlx as Driver>::error(Error::decode(
                index,
                "a negative MySQL integer cannot be decoded as u64",
            ))
        })
    }
}

/// The digits of a `DECIMAL` column, when MySQL sent the value as a plain integer.
///
/// `BigDecimal`'s `Decode` parses that text into a `BigInt`, which allocates a digit vector,
/// and `to_u128`/`to_i128` then walk it again. MySQL already sends a decimal as text, so
/// parsing it straight into a 128-bit integer is the same check with no intermediate
/// allocation. Only a plain integer — an optional `-` followed by digits — takes this route:
/// a scaled value has to keep going through `BigDecimal`, which truncates it, so that the two
/// paths agree on what a fractional value means.
fn decimal_integer(row: &MySqlRow, index: usize) -> Option<(&str, bool)> {
    let value = row.try_get_raw(index).ok()?;
    if value.is_null() || !<BigDecimal as Type<MySql>>::compatible(value.type_info().as_ref()) {
        return None;
    }

    let text = <&str as sqlx::Decode<MySql>>::decode(value).ok()?;
    let (negative, digits) = match text.strip_prefix('-') {
        Some(digits) => (true, digits),
        None => (false, text),
    };

    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((digits, negative))
}

/// Parse decimal digits into a `u128`, returning `None` on overflow.
fn parse_u128(digits: &str) -> Option<u128> {
    let mut value = 0_u128;
    for byte in digits.bytes() {
        let digit = byte.checked_sub(b'0')?;
        if digit > 9 {
            return None;
        }
        value = value.checked_mul(10)?.checked_add(u128::from(digit))?;
    }
    Some(value)
}

/// Parse decimal digits and a sign into an `i128`, returning `None` on overflow.
fn parse_i128(digits: &str, negative: bool) -> Option<i128> {
    let magnitude = parse_u128(digits)?;
    if negative {
        // `i128::MIN`'s magnitude is one past `i128::MAX`, so go through the unsigned
        // two's-complement value rather than negating arithmetically.
        (magnitude <= i128::MAX as u128 + 1).then(|| (magnitude as i128).wrapping_neg())
    } else {
        i128::try_from(magnitude).ok()
    }
}

impl Decode<Sqlx> for u128 {
    fn decode(row: &MySqlRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;

        // A `DECIMAL(39,0)` column, which is what a `u128` column is declared as, takes this
        // path. A negative or scaled value falls through to `BigDecimal` so its diagnosis is
        // unchanged.
        if let Some((digits, false)) = decimal_integer(row, index) {
            *offset += 1;
            return parse_u128(digits).ok_or_else(|| {
                <Sqlx as Driver>::error(Error::decode(
                    index,
                    "the DECIMAL value is not a non-negative integer that fits in u128",
                ))
            });
        }

        // `to_u128` returns `None` for a negative value and for anything above `u128::MAX`.
        if let Ok(value) = row.try_get::<BigDecimal, _>(index) {
            *offset += 1;
            return value.to_u128().ok_or_else(|| {
                <Sqlx as Driver>::error(Error::decode(
                    index,
                    "the DECIMAL value is not a non-negative integer that fits in u128",
                ))
            });
        }

        // A value held in a real integer column still decodes through the integer path.
        let value = decode_integer(row, offset)?;
        u128::try_from(value).map_err(|_| {
            <Sqlx as Driver>::error(Error::decode(
                index,
                "a negative MySQL integer cannot be decoded as u128",
            ))
        })
    }
}

impl Decode<Sqlx> for i128 {
    fn decode(row: &MySqlRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;

        if let Some((digits, negative)) = decimal_integer(row, index) {
            *offset += 1;
            return parse_i128(digits, negative).ok_or_else(|| {
                <Sqlx as Driver>::error(Error::decode(
                    index,
                    "the DECIMAL value is not an integer that fits in i128",
                ))
            });
        }

        if let Ok(value) = row.try_get::<BigDecimal, _>(index) {
            *offset += 1;
            return value.to_i128().ok_or_else(|| {
                <Sqlx as Driver>::error(Error::decode(
                    index,
                    "the DECIMAL value is not an integer that fits in i128",
                ))
            });
        }

        decode_integer(row, offset).map(i128::from)
    }
}

impl Decode<Sqlx> for serde_json::Value {
    fn decode(row: &MySqlRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let value: sqlx::types::Json<serde_json::Value> = decode_via_sqlx(row, offset)?;
        Ok(value.0)
    }
}
