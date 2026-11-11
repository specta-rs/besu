use besu_core::{Decode, Driver, Error, Sql, TransactionDriver, Value};
use sqlx::{
    Arguments, MySql, MySqlPool, Row,
    mysql::{MySqlArguments, MySqlConnectOptions, MySqlPoolOptions, MySqlQueryResult, MySqlRow},
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

fn arguments(sql: &Sql<MySqlDialect>) -> Result<MySqlArguments, sqlx::Error> {
    let mut arguments = MySqlArguments::default();
    for value in sql.args() {
        match value {
            Value::Null => add(&mut arguments, None::<i64>)?,
            Value::Bool(v) => add(&mut arguments, *v)?,
            Value::Int(v) => add(&mut arguments, *v)?,
            // MySQL has real unsigned integers, so the full `u64` range round-trips.
            Value::UInt(v) => add(&mut arguments, *v)?,
            Value::Float(v) => add(&mut arguments, *v)?,
            Value::Text(v) => add(&mut arguments, v.as_str())?,
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
            _ => add(&mut arguments, None::<i64>)?,
        }
    }
    Ok(arguments)
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
/// through a signed type. Trying signed first and unsigned second covers both without
/// looking at the column metadata.
fn decode_integer(row: &MySqlRow, offset: &mut usize) -> Result<i64, sqlx::Error> {
    let index = *offset;

    if let Ok(value) = row.try_get::<i64, _>(index) {
        *offset += 1;
        return Ok(value);
    }

    if let Ok(value) = row.try_get::<u64, _>(index) {
        *offset += 1;
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
        // decodes as `i64`, so try both rather than guessing from the value.
        if let Ok(value) = row.try_get::<u64, _>(index) {
            *offset += 1;
            return Ok(value);
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

impl Decode<Sqlx> for serde_json::Value {
    fn decode(row: &MySqlRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let value: sqlx::types::Json<serde_json::Value> = decode_via_sqlx(row, offset)?;
        Ok(value.0)
    }
}
