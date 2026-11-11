use std::{path::Path, str::from_utf8};

use besu_core::{Decode, Driver, Error, Sql, TransactionDriver, Value};
use sqlx::{
    Arguments, Row, Sqlite, SqlitePool,
    sqlite::{
        SqliteArguments, SqliteConnectOptions, SqlitePoolOptions, SqliteQueryResult, SqliteRow,
    },
};

use crate::SQLite;

/// A [`besu_core::Driver`] for SQLite, backed by [`sqlx`].
///
/// The pool is created with sensible defaults: foreign keys enforced, WAL journalling, and
/// the database file created if it does not exist.
#[derive(Debug, Clone)]
pub struct Sqlx {
    pool: SqlitePool,
}

impl Sqlx {
    /// Open (or create) a SQLite database file.
    pub async fn new(path: impl AsRef<Path>) -> Result<Self, sqlx::Error> {
        let path = path.as_ref();
        Self::with_options(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true)
                .foreign_keys(true)
                .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal),
        )
        .await
    }

    /// Open a database from a connection string such as `sqlite::memory:`.
    pub async fn connect(url: &str) -> Result<Self, sqlx::Error> {
        let options = url.parse::<SqliteConnectOptions>()?;
        Self::with_options(options).await
    }

    /// Open an in-memory database.
    pub async fn memory() -> Result<Self, sqlx::Error> {
        Self::with_options(SqliteConnectOptions::new().in_memory(true)).await
    }

    async fn with_options(options: SqliteConnectOptions) -> Result<Self, sqlx::Error> {
        let in_memory = options
            .get_filename()
            .to_str()
            .is_some_and(|name| name == ":memory:");

        let mut pool = SqlitePoolOptions::new();
        if in_memory {
            // `:memory:` only stays alive for one connection, and a pool would otherwise
            // open several databases that do not see each other's tables.
            pool = pool
                .max_connections(1)
                .idle_timeout(None)
                .max_lifetime(None);
        }

        Ok(Self {
            pool: pool.connect_with(options).await?,
        })
    }

    /// Wrap an existing pool.
    pub fn from_pool(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// The underlying pool, for anything Besu does not cover.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

fn add<'q, T: 'q>(arguments: &mut SqliteArguments<'q>, value: T) -> Result<(), sqlx::Error>
where
    T: sqlx::Encode<'q, Sqlite> + sqlx::Type<Sqlite>,
{
    arguments.add(value).map_err(sqlx::Error::Encode)
}

fn arguments<'q>(sql: &'q Sql<SQLite>) -> Result<SqliteArguments<'q>, sqlx::Error> {
    let mut arguments = SqliteArguments::default();
    for value in sql.args() {
        match value {
            Value::Null => add(&mut arguments, None::<i64>)?,
            Value::Bool(v) => add(&mut arguments, *v)?,
            Value::Int(v) => add(&mut arguments, *v)?,
            Value::UInt(v) => add(&mut arguments, i64::try_from(*v).unwrap_or(i64::MAX))?,
            Value::Float(v) => add(&mut arguments, *v)?,
            Value::Text(v) => add(&mut arguments, v.as_str())?,
            Value::Bytes(v) => add(&mut arguments, v.as_slice())?,
            Value::Uuid(v) => add(&mut arguments, *v)?,
            Value::Date(v) => add(&mut arguments, *v)?,
            Value::Time(v) => add(&mut arguments, *v)?,
            Value::Timestamp(v) => add(&mut arguments, *v)?,
            Value::TimestampTz(v) => add(&mut arguments, *v)?,
            Value::Json(v) => add(&mut arguments, from_utf8(v).unwrap_or("null"))?,
            _ => add(&mut arguments, None::<i64>)?,
        }
    }
    Ok(arguments)
}

impl Driver for Sqlx {
    type Dialect = SQLite;
    type Error = sqlx::Error;
    type Row = SqliteRow;
    type Output = SqliteQueryResult;

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

    async fn query(&self, sql: &Sql<SQLite>) -> Result<Vec<Self::Row>, Self::Error> {
        let arguments = arguments(sql)?;
        sqlx::query_with(sql.sql(), arguments)
            .fetch_all(&self.pool)
            .await
    }

    async fn execute(&self, sql: &Sql<SQLite>) -> Result<Self::Output, Self::Error> {
        let arguments = arguments(sql)?;
        sqlx::query_with(sql.sql(), arguments)
            .execute(&self.pool)
            .await
    }
}

impl besu::SchemaDriver for Sqlx {
    type Ddl = crate::SQLiteDdl;

    async fn introspect(&self) -> Result<besu::schema::Schema, Self::Error> {
        let db = besu::Database::new(self.clone());
        crate::introspect::introspect(&db).await
    }
}

impl TransactionDriver for Sqlx {
    type Transaction<'a> = sqlx::Transaction<'a, Sqlite>;

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
        sql: &Sql<SQLite>,
    ) -> Result<Vec<Self::Row>, Self::Error> {
        let arguments = arguments(sql)?;
        sqlx::query_with(sql.sql(), arguments)
            .fetch_all(&mut **tx)
            .await
    }

    async fn txn_execute<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &Sql<SQLite>,
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

fn decode_via_sqlx<T>(row: &SqliteRow, offset: &mut usize) -> Result<T, sqlx::Error>
where
    T: for<'r> sqlx::Decode<'r, Sqlite> + sqlx::Type<Sqlite>,
{
    let index = *offset;
    *offset += 1;
    row.try_get(index)
}

macro_rules! impl_decode {
    ($($ty:ty),* $(,)?) => {
        $(
            impl Decode<Sqlx> for $ty {
                fn decode(row: &SqliteRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
                    decode_via_sqlx(row, offset)
                }
            }
        )*
    };
}

impl_decode!(
    bool,
    i8,
    i16,
    i32,
    i64,
    u8,
    u16,
    u32,
    String,
    Vec<u8>,
    f64,
    uuid::Uuid,
    chrono::NaiveDate,
    chrono::NaiveTime,
    chrono::NaiveDateTime,
    chrono::DateTime<chrono::Utc>,
);

impl Decode<Sqlx> for f32 {
    fn decode(row: &SqliteRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let value: f64 = decode_via_sqlx(row, offset)?;
        Ok(value as f32)
    }
}

impl Decode<Sqlx> for u64 {
    fn decode(row: &SqliteRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let value: i64 = decode_via_sqlx(row, offset)?;
        u64::try_from(value).map_err(|_| {
            <Sqlx as Driver>::error(Error::decode(
                *offset - 1,
                "a negative SQLite integer cannot be decoded as u64",
            ))
        })
    }
}

impl Decode<Sqlx> for serde_json::Value {
    fn decode(row: &SqliteRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let value: sqlx::types::Json<serde_json::Value> = decode_via_sqlx(row, offset)?;
        Ok(value.0)
    }
}
