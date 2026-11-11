use besu_core::{Decode, Driver, Error, Sql, TransactionDriver, Value};
use sqlx::{
    Arguments, PgPool, Postgres as SqlxPostgres, Row,
    postgres::{PgArgumentBuffer, PgArguments, PgQueryResult, PgRow},
};

use crate::Postgres;

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
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        sqlx::postgres::PgTypeInfo::with_oid(sqlx::postgres::types::Oid(0))
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

fn add<'q, T>(arguments: &mut PgArguments, value: T) -> Result<(), sqlx::Error>
where
    T: sqlx::Encode<'q, SqlxPostgres> + sqlx::Type<SqlxPostgres>,
{
    arguments.add(value).map_err(sqlx::Error::Encode)
}

fn arguments(sql: &Sql<Postgres>) -> Result<PgArguments, sqlx::Error> {
    let mut arguments = PgArguments::default();
    for value in sql.args() {
        match value {
            Value::Null => add(&mut arguments, None::<UntypedNull>)?,
            Value::Bool(v) => add(&mut arguments, *v)?,
            Value::Int(v) => add(&mut arguments, *v)?,
            Value::UInt(v) => {
                // TODO: `u64` values above `i64::MAX` are stored as `NUMERIC(20,0)`, but sqlx
                // cannot encode or decode PostgreSQL's `NUMERIC` without a decimal feature
                // enabled, so refuse them loudly rather than write a truncated value.
                let v = i64::try_from(*v).map_err(|_| {
                    <Sqlx as Driver>::error(Error::encode(
                        v,
                        "this driver cannot encode an unsigned integer above `i64::MAX` yet",
                    ))
                })?;
                add(&mut arguments, v)?;
            }
            Value::Float(v) => add(&mut arguments, *v)?,
            Value::Text(v) => add(&mut arguments, v.as_str())?,
            Value::Bytes(v) => add(&mut arguments, v.as_slice())?,
            Value::Uuid(v) => add(&mut arguments, *v)?,
            Value::Date(v) => add(&mut arguments, *v)?,
            Value::Time(v) => add(&mut arguments, *v)?,
            Value::Timestamp(v) => add(&mut arguments, *v)?,
            Value::TimestampTz(v) => add(&mut arguments, *v)?,
            Value::Json(v) => {
                let json: serde_json::Value = serde_json::from_slice(v).map_err(|err| {
                    <Sqlx as Driver>::error(Error::encode("json", err.to_string()))
                })?;
                add(&mut arguments, sqlx::types::Json(json))?;
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
            .map(|value| sqlx::ValueRef::is_null(&value))
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
        // TODO: values above `i64::MAX` live in a `NUMERIC(20,0)` column, which this driver
        // cannot decode until sqlx is built with a decimal feature.
        let value: i64 = decode_via_sqlx(row, offset)?;
        u64::try_from(value).map_err(|_| {
            <Sqlx as Driver>::error(Error::decode(
                index,
                "a negative PostgreSQL integer cannot be decoded as `u64`",
            ))
        })
    }
}

impl Decode<Sqlx> for f32 {
    fn decode(row: &PgRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let value: f64 = decode_via_sqlx(row, offset)?;
        Ok(value as f32)
    }
}

impl Decode<Sqlx> for serde_json::Value {
    fn decode(row: &PgRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let value: sqlx::types::Json<serde_json::Value> = decode_via_sqlx(row, offset)?;
        Ok(value.0)
    }
}
