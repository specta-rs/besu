//! A PostgreSQL driver backed by [`tokio_postgres`], behind the `native` feature.
//!
//! The pool is a [`deadpool_postgres::Pool`]: an `Arc` of idle connections with a maximum
//! size of ten, opened lazily and health-checked when they are handed out. No connection is
//! held behind a mutex, so concurrent queries use different connections.
//!
//! # TLS
//!
//! This driver uses [`tokio_postgres::NoTls`]. TLS is the API decision the issue left open:
//! exposing it means a feature that pulls in `rustls` (or `native-tls`), a connector argument
//! on every constructor, and a story for `sslmode` in the URL. Until that exists, a URL that
//! requires TLS fails to connect rather than silently going unencrypted.
//!
//! # Transactions
//!
//! A transaction borrows one pooled connection and issues `BEGIN`, `COMMIT` and `ROLLBACK`
//! itself. That is the same shape sqlx gives, and it means nested `tx.transaction(..)` calls
//! are `SAVEPOINT` statements like they are on every other Besu driver. Dropping a
//! transaction without committing detaches its connection from the pool: the socket closes,
//! PostgreSQL rolls the transaction back, and the next borrower is never handed a connection
//! that is still inside one.
//!
//! # Streaming
//!
//! [`Driver::query_stream`] is a real stream: the rows arrive as the server sends them and
//! the pooled connection stays checked out until the stream is dropped. A statement inside a
//! transaction streams from that transaction's connection instead, which is why the stream
//! borrows the transaction.
//!
//! # The binary format
//!
//! PostgreSQL's binary formats are a property of the server, not of the driver, so the
//! encoders and parsers here mirror the sqlx driver's: `NUMERIC` is base-10,000 groups,
//! arrays carry a dimension header and an element OID, ranges carry a flag byte, and the
//! network, interval, bit, EWKB and vector layouts are unchanged. Keeping the port separate
//! means the native driver has no sqlx in its dependency graph; the unit tests at the bottom
//! of this file pin the parts that are easiest to get subtly wrong (a `NUMERIC` with a gap
//! before its first digit, an empty range, a malformed payload).

use std::{
    error::Error as StdError,
    fmt,
    future::poll_fn,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
};

use besu_core::{
    BitString, Bound, Cidr, Decode, Driver, Error, Geometry, Inet, Range, RowStream, Sql,
    SqlInterval, Stream, TransactionDriver, Value, Vector,
};
use bytes::{BufMut, BytesMut};
use chrono::Timelike as _;
use deadpool_postgres::{Manager, ManagerConfig, Object, Pool, RecyclingMethod, Runtime};
use tokio_postgres::types::{FromSql, IsNull, Kind, ToSql, Type};

use crate::Postgres;

// ---------------------------------------------------------------------------------------
// Stable OIDs
// ---------------------------------------------------------------------------------------

/// `BOOL`.
const BOOL_OID: u32 = 16;
/// `BYTEA`.
const BYTEA_OID: u32 = 17;
/// `"char"`.
const CHAR_OID: u32 = 18;
/// `NAME`.
const NAME_OID: u32 = 19;
/// `INT8`.
const INT8_OID: u32 = 20;
/// `INT2`.
const INT2_OID: u32 = 21;
/// `INT4`.
const INT4_OID: u32 = 23;
/// `TEXT`.
const TEXT_OID: u32 = 25;
/// `OID`.
const OID_OID: u32 = 26;
/// `JSON`.
const JSON_OID: u32 = 114;
/// `CIDR`.
const CIDR_OID: u32 = 650;
/// `FLOAT4`.
const FLOAT4_OID: u32 = 700;
/// `FLOAT8`.
const FLOAT8_OID: u32 = 701;
/// `INET`.
const INET_OID: u32 = 869;
/// `BPCHAR`.
const BPCHAR_OID: u32 = 1042;
/// `VARCHAR`.
const VARCHAR_OID: u32 = 1043;
/// `DATE`.
const DATE_OID: u32 = 1082;
/// `TIME`.
const TIME_OID: u32 = 1083;
/// `TIMESTAMP`.
const TIMESTAMP_OID: u32 = 1114;
/// `TIMESTAMPTZ`.
const TIMESTAMPTZ_OID: u32 = 1184;
/// `INTERVAL`.
const INTERVAL_OID: u32 = 1186;
/// `BIT`.
const BIT_OID: u32 = 1560;
/// `VARBIT`.
const VARBIT_OID: u32 = 1562;
/// `NUMERIC`.
const NUMERIC_OID: u32 = 1700;
/// `UUID`.
const UUID_OID: u32 = 2950;
/// `JSONB`.
const JSONB_OID: u32 = 3802;

// ---------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------

/// Everything the native PostgreSQL driver can fail with.
#[derive(Debug)]
#[non_exhaustive]
pub enum NativeError {
    /// The server rejected a statement, or the connection failed.
    Postgres(tokio_postgres::Error),
    /// The connection pool could not hand out a connection.
    Pool(deadpool_postgres::PoolError),
    /// The pool itself could not be built, usually because no runtime was chosen.
    PoolBuild(deadpool_postgres::BuildError),
    /// A value could not be encoded, or a column could not be decoded.
    Core(Error),
}

impl fmt::Display for NativeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // `tokio-postgres` puts the interesting part of a failed encode or decode in
            // the source chain, so the message walks it. Without this, a rejected JSON
            // parameter reads only as "error serializing parameter 0".
            Self::Postgres(err) => write_chain(f, err),
            Self::Pool(err) => write_chain(f, err),
            Self::PoolBuild(err) => write!(f, "could not build the PostgreSQL pool: {err}"),
            Self::Core(err) => write!(f, "{err}"),
        }
    }
}

/// Display an error and every cause behind it, separated by `: `.
fn write_chain(f: &mut fmt::Formatter<'_>, err: &dyn StdError) -> fmt::Result {
    write!(f, "{err}")?;
    let mut source = err.source();
    while let Some(cause) = source {
        write!(f, ": {cause}")?;
        source = cause.source();
    }
    Ok(())
}

impl StdError for NativeError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Postgres(err) => Some(err),
            Self::Pool(err) => Some(err),
            Self::PoolBuild(err) => Some(err),
            Self::Core(err) => Some(err),
        }
    }
}

impl From<tokio_postgres::Error> for NativeError {
    fn from(err: tokio_postgres::Error) -> Self {
        Self::Postgres(err)
    }
}

impl From<deadpool_postgres::PoolError> for NativeError {
    fn from(err: deadpool_postgres::PoolError) -> Self {
        Self::Pool(err)
    }
}

impl From<deadpool_postgres::BuildError> for NativeError {
    fn from(err: deadpool_postgres::BuildError) -> Self {
        Self::PoolBuild(err)
    }
}

impl From<Error> for NativeError {
    fn from(err: Error) -> Self {
        Self::Core(err)
    }
}

/// What a statement that returns no rows reported.
///
/// `tokio-postgres` exposes the affected-row count but not the command tag, so unlike the
/// sqlx driver this carries only the count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NativeOutput {
    rows_affected: u64,
}

impl NativeOutput {
    /// The number of rows the statement changed.
    pub const fn rows_affected(&self) -> u64 {
        self.rows_affected
    }
}

/// One row of a native PostgreSQL result set.
///
/// The row owns its statement handle and its data, so it outlives the connection that
/// produced it. The column names are read from that handle rather than copied into every
/// row.
#[derive(Clone, Debug)]
pub struct NativeRow {
    inner: tokio_postgres::Row,
}

impl NativeRow {
    /// The number of values in the row.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the row has no values.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Whether the value at `index` is `NULL`.
    ///
    /// An out-of-range index reports `true`, matching [`Driver::is_null`].
    pub fn is_null(&self, index: usize) -> bool {
        index >= self.len()
            || self
                .inner
                .try_get::<_, Option<RawBytes>>(index)
                .map(|value| value.is_none())
                .unwrap_or(true)
    }

    /// The names of the row's columns, in decode order.
    pub fn column_names(&self) -> Vec<String> {
        self.inner
            .columns()
            .iter()
            .map(|column| column.name().to_owned())
            .collect()
    }

    /// The SQL type name of the column at `index`.
    pub fn column_type(&self, index: usize) -> Option<&str> {
        self.inner
            .columns()
            .get(index)
            .map(|column| base_type(column.type_()).name())
    }
}

// ---------------------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------------------

/// A PostgreSQL driver backed by [`tokio_postgres`] and a `deadpool` connection pool.
///
/// The constructors take a libpq URL, are async because the connection is checked eagerly,
/// and require no TLS configuration: the driver uses [`tokio_postgres::NoTls`].
#[derive(Debug, Clone)]
pub struct Native {
    pool: Pool,
}

impl Native {
    /// Open a connection pool from a PostgreSQL connection string.
    ///
    /// This is an alias for [`Native::connect`], for symmetry with the other Besu drivers.
    pub async fn new(url: impl AsRef<str>) -> Result<Self, NativeError> {
        Self::connect(url.as_ref()).await
    }

    /// Open a connection pool from a connection string such as
    /// `postgres://user:password@localhost/database`.
    ///
    /// One connection is opened before returning, so an unreachable server is an error from
    /// the constructor rather than from the first query. The pool holds at most ten
    /// connections, the same default as `Sqlx`.
    pub async fn connect(url: &str) -> Result<Self, NativeError> {
        let config: tokio_postgres::Config = url.parse()?;
        let manager = Manager::from_config(
            config,
            tokio_postgres::NoTls,
            ManagerConfig {
                recycling_method: RecyclingMethod::Fast,
            },
        );
        let pool = Pool::builder(manager)
            .max_size(10)
            .runtime(Runtime::Tokio1)
            .build()?;

        // Check the server is there now, so a typo in a URL is not reported by whichever
        // query happens to run first.
        let client = pool.get().await?;
        drop(client);

        Ok(Self { pool })
    }

    /// Wrap an existing pool.
    pub fn from_pool(pool: Pool) -> Self {
        Self { pool }
    }

    /// The underlying pool, for anything Besu does not cover.
    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    /// Wrap this driver in a [`besu::Database`].
    pub fn database(self) -> besu::Database<Self> {
        besu::Database::new(self)
    }
}

impl Driver for Native {
    type Dialect = Postgres;
    type Error = NativeError;
    type Row = NativeRow;
    type Output = NativeOutput;

    fn error(err: Error) -> Self::Error {
        NativeError::Core(err)
    }

    fn row_len(row: &Self::Row) -> usize {
        row.len()
    }

    fn is_null(row: &Self::Row, index: usize) -> bool {
        row.is_null(index)
    }

    fn column_names(row: &Self::Row) -> Option<Vec<String>> {
        Some(row.column_names())
    }

    fn decode_text(row: &Self::Row, index: usize) -> Result<String, Self::Error> {
        let raw = row
            .inner
            .try_get::<_, Option<RawBytes>>(index)
            .map_err(|err| decode_error(index, err.to_string()))?;
        let bytes = raw
            .ok_or_else(|| decode_error(index, "the column is NULL"))?
            .0;
        core::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| decode_error(index, "the value is not valid UTF-8"))
    }

    async fn query(&self, sql: &Sql<Postgres>) -> Result<Vec<Self::Row>, Self::Error> {
        let client = self.pool.get().await?;
        let params = arguments(sql);
        let rows = client
            .query_raw(
                sql.sql(),
                params.iter().map(|param| param as &(dyn ToSql + Sync)),
            )
            .await?;
        collect(rows).await
    }

    async fn execute(&self, sql: &Sql<Postgres>) -> Result<Self::Output, Self::Error> {
        let client = self.pool.get().await?;
        let params = arguments(sql);
        let rows_affected = client
            .execute_raw(
                sql.sql(),
                params.iter().map(|param| param as &(dyn ToSql + Sync)),
            )
            .await?;
        Ok(NativeOutput { rows_affected })
    }

    async fn query_stream<'a>(
        &'a self,
        sql: &'a Sql<Postgres>,
    ) -> Result<RowStream<'a, Self>, Self::Error> {
        let client = self.pool.get().await?;
        let rows = start_stream(&client, sql).await?;
        Ok(Box::pin(NativeRowStream {
            rows,
            _client: Some(client),
        }) as RowStream<'a, Self>)
    }
}

/// Run a statement and collect every row.
async fn collect(rows: tokio_postgres::RowStream) -> Result<Vec<NativeRow>, NativeError> {
    let mut rows = core::pin::pin!(rows);
    let mut out = Vec::new();
    while let Some(row) = poll_fn(|cx| rows.as_mut().poll_next(cx)).await {
        out.push(NativeRow { inner: row? });
    }
    Ok(out)
}

/// Prepare a statement and start its row stream.
///
/// The parameters live only until `query_raw` returns: the rows are already on their way by
/// then, so the statement is encoded exactly once and no bound value is held any longer.
async fn start_stream(
    client: &deadpool_postgres::Client,
    sql: &Sql<Postgres>,
) -> Result<Pin<Box<tokio_postgres::RowStream>>, NativeError> {
    let params = arguments(sql);
    let rows = client
        .query_raw(
            sql.sql(),
            params.iter().map(|param| param as &(dyn ToSql + Sync)),
        )
        .await?;
    Ok(Box::pin(rows))
}

/// A stream that owns the pooled connection and the server's row stream.
///
/// The client is only there to keep the connection checked out: dropping it while the server
/// is still sending rows would hand a busy connection to the next query.
struct NativeRowStream {
    // Declared first so the stream is dropped before the connection.
    rows: Pin<Box<tokio_postgres::RowStream>>,
    _client: Option<Object>,
}

impl Stream for NativeRowStream {
    type Item = Result<NativeRow, NativeError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rows.as_mut().poll_next(cx).map(|row| {
            row.map(|row| {
                row.map(|inner| NativeRow { inner })
                    .map_err(NativeError::from)
            })
        })
    }
}

// ---------------------------------------------------------------------------------------
// Transactions
// ---------------------------------------------------------------------------------------

/// A transaction on one pooled connection.
///
/// Dropping the handle without committing detaches the connection from the pool, which
/// closes it; PostgreSQL rolls the open transaction back as the socket goes away.
pub struct NativeTransaction<'a> {
    client: Option<Object>,
    finished: bool,
    marker: PhantomData<&'a Native>,
}

impl fmt::Debug for NativeTransaction<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeTransaction")
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl Drop for NativeTransaction<'_> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if let Some(client) = self.client.take() {
            // `take` removes the connection from the pool's circulation. Its destructor
            // aborts the connection task, so the server sees a disconnect and rolls back.
            // Returning it instead would hand an open transaction to the next borrower.
            drop(Object::take(client));
        }
    }
}

impl NativeTransaction<'_> {
    fn client(&self) -> Result<&Object, NativeError> {
        self.client.as_ref().ok_or_else(|| {
            NativeError::Core(Error::invalid_query("the transaction is no longer open"))
        })
    }
}

impl TransactionDriver for Native {
    type Transaction<'a> = NativeTransaction<'a>;

    async fn begin<'a>(&'a self) -> Result<Self::Transaction<'a>, Self::Error> {
        let client = self.pool.get().await?;
        client.batch_execute("BEGIN").await?;
        Ok(NativeTransaction {
            client: Some(client),
            finished: false,
            marker: PhantomData,
        })
    }

    async fn commit<'a>(mut tx: Self::Transaction<'a>) -> Result<(), Self::Error> {
        tx.finished = true;
        match tx.client.take() {
            Some(client) => {
                let result = client.batch_execute("COMMIT").await;
                if result.is_err() {
                    // The connection may be broken, or still inside the transaction. Do not
                    // give it back to the pool in either state.
                    drop(Object::take(client));
                }
                result.map_err(Into::into)
            }
            None => Ok(()),
        }
    }

    async fn rollback<'a>(mut tx: Self::Transaction<'a>) -> Result<(), Self::Error> {
        tx.finished = true;
        match tx.client.take() {
            Some(client) => {
                let result = client.batch_execute("ROLLBACK").await;
                if result.is_err() {
                    drop(Object::take(client));
                }
                result.map_err(Into::into)
            }
            None => Ok(()),
        }
    }

    async fn txn_query<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &Sql<Postgres>,
    ) -> Result<Vec<Self::Row>, Self::Error> {
        let client = tx.client()?;
        let params = arguments(sql);
        let rows = client
            .query_raw(
                sql.sql(),
                params.iter().map(|param| param as &(dyn ToSql + Sync)),
            )
            .await?;
        collect(rows).await
    }

    async fn txn_execute<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &Sql<Postgres>,
    ) -> Result<Self::Output, Self::Error> {
        let client = tx.client()?;
        let params = arguments(sql);
        let rows_affected = client
            .execute_raw(
                sql.sql(),
                params.iter().map(|param| param as &(dyn ToSql + Sync)),
            )
            .await?;
        Ok(NativeOutput { rows_affected })
    }

    async fn txn_query_stream<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &'c Sql<Postgres>,
    ) -> Result<RowStream<'c, Self>, Self::Error> {
        let client = tx.client()?;
        let rows = start_stream(client, sql).await?;
        // The stream borrows nothing, but the transaction must stay alive while it drains:
        // the connection it uses is owned by the transaction, and the returned lifetime is
        // what enforces that.
        Ok(Box::pin(NativeRowStream {
            rows,
            _client: None,
        }) as RowStream<'c, Self>)
    }
}

// ---------------------------------------------------------------------------------------
// `SchemaDriver`
// ---------------------------------------------------------------------------------------

impl besu::SchemaDriver for Native {
    type Ddl = crate::PostgresDdl;

    async fn introspect(&self) -> Result<besu::schema::Schema, Self::Error> {
        let db = besu::Database::new(self.clone());
        crate::introspect::introspect(&db).await
    }

    /// Run every schema statement on one connection.
    ///
    /// `push` is deliberately not transactional, but it is one logical change, and a
    /// statement that depends on a setting another statement made should see the same
    /// session. Taking one pooled connection for the whole list is the cheapest way to
    /// guarantee that without pretending the list is atomic.
    async fn execute_statements(&self, statements: &[String]) -> Result<(), Self::Error> {
        let client = self.pool.get().await?;
        for statement in statements {
            client.batch_execute(statement).await?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------------------

/// A bound parameter, written in the binary format of whatever type the server inferred.
///
/// The SQL text does not declare a type for a placeholder; the server infers one from the
/// column or the cast around it. That inferred type reaches this encoder, which is why the
/// same `Value` can be written as `INT2`, `INT4`, `INT8` or `NUMERIC`. sqlx declared a type
/// per Rust type and relied on PostgreSQL's assignment casts; writing the inferred type
/// needs no cast and no second parameter type for one SQL string.
#[derive(Debug, Clone, Copy)]
struct Param<'a>(&'a Value);

impl ToSql for Param<'_> {
    fn to_sql(
        &self,
        ty: &Type,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn StdError + Sync + Send>> {
        encode(self.0, ty, out).map_err(|reason| {
            Box::new(std::io::Error::other(reason)) as Box<dyn StdError + Sync + Send>
        })?;
        Ok(if self.0.is_null() {
            IsNull::Yes
        } else {
            IsNull::No
        })
    }

    fn accepts(_: &Type) -> bool {
        // The value decides, in `encode`; a static `accepts` cannot see it. Every type is
        // accepted here and an incompatible pair is a precise encode error instead.
        true
    }

    fn to_sql_checked(
        &self,
        ty: &Type,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn StdError + Sync + Send>> {
        self.to_sql(ty, out)
    }
}

/// Encode a statement's values as pooled parameters.
fn arguments(sql: &Sql<Postgres>) -> Vec<Param<'_>> {
    sql.args().iter().map(Param).collect()
}

/// The type behind a domain, or the type itself when it is not one.
fn base_type(ty: &Type) -> &Type {
    let mut current = ty;
    while let Kind::Domain(inner) = current.kind() {
        current = inner;
    }
    current
}

/// The OID of a value's real type, reaching through a domain.
fn type_oid(ty: &Type) -> u32 {
    base_type(ty).oid()
}

fn encode(value: &Value, ty: &Type, out: &mut BytesMut) -> Result<(), String> {
    match value {
        Value::Null => {}
        Value::Bool(value) => out.put_u8(u8::from(*value)),
        Value::Int(value) => write_signed_integer(i128::from(*value), ty, out)?,
        Value::UInt(value) => write_unsigned_integer(u128::from(*value), ty, out)?,
        Value::UInt128(value) => write_unsigned_integer(*value, ty, out)?,
        Value::Int128(value) => write_signed_integer(*value, ty, out)?,
        Value::Float(value) => match type_oid(ty) {
            FLOAT4_OID => out.put_f32(*value as f32),
            FLOAT8_OID => out.put_f64(*value),
            oid => return Err(type_mismatch("a float", oid)),
        },
        Value::Text(value) => write_text(value, ty, out)?,
        Value::EnumLabel(value) => out.put_slice(value.as_bytes()),
        Value::Bytes(value) => out.put_slice(value),
        Value::Uuid(value) => out.put_slice(value.as_bytes()),
        Value::Date(value) => {
            let days = value.signed_duration_since(postgres_epoch()).num_days();
            let days = i32::try_from(days).map_err(|_| "the date is out of range".to_owned())?;
            out.put_i32(days);
        }
        Value::Time(value) => {
            let micros = i64::from(value.num_seconds_from_midnight()) * 1_000_000
                + i64::from(value.nanosecond() / 1_000);
            out.put_i64(micros);
        }
        Value::Timestamp(value) => out.put_i64(micros_since_epoch(*value)?),
        Value::TimestampTz(value) => out.put_i64(micros_since_epoch(value.naive_utc())?),
        Value::Decimal(value) => write_numeric_text(value, out)?,
        Value::Json(value) => {
            // Validate in place. The document is already JSON text, so parsing it into a
            // tree only to serialise it back would be two allocations for no change.
            let _: &serde_json::value::RawValue = serde_json::from_slice(value)
                .map_err(|err| Error::encode("json", err.to_string()).to_string())?;
            if type_oid(ty) == JSONB_OID {
                // The binary JSONB form is a one-byte version followed by the document.
                out.put_u8(1);
            }
            out.put_slice(value);
        }
        Value::Interval(value) => {
            // `interval_send` writes the time in microseconds, then whole days, then whole
            // months, in that order.
            out.put_i64(value.microseconds());
            out.put_i32(value.days());
            out.put_i32(value.months());
        }
        Value::Inet(value) => write_network(&NetworkValue::inet(value), out),
        Value::Cidr(value) => write_network(&NetworkValue::cidr(value), out),
        Value::Bit(value) => {
            let length =
                i32::try_from(value.len()).map_err(|_| "the bit string is too long".to_owned())?;
            out.put_i32(length);
            out.put_slice(&value.to_bytes());
        }
        Value::Range { range, .. } => {
            let element = match base_type(ty).kind() {
                Kind::Range(element) => element,
                _ => return Err(type_mismatch("a range", type_oid(ty))),
            };
            write_range(element, range, out)?;
        }
        Value::Array { values, .. } => write_array(values, ty, out)?,
        Value::Geometry(value) => out.put_slice(value.ewkb()),
        Value::Vector(value) => out.put_slice(&value.to_binary()),
        // `Value` is `#[non_exhaustive]`: a future variant fails loudly rather than
        // silently becoming `NULL`.
        other => {
            return Err(Error::encode(
                other.display(),
                "this driver does not know how to bind this value",
            )
            .to_string());
        }
    }
    Ok(())
}

fn type_mismatch(what: &str, oid: u32) -> String {
    format!("{what} cannot be written as the parameter type with OID {oid}")
}

/// Write a signed integer in whatever width the server inferred.
fn write_signed_integer(value: i128, ty: &Type, out: &mut BytesMut) -> Result<(), String> {
    match type_oid(ty) {
        INT2_OID => out.put_i16(
            i16::try_from(value).map_err(|_| "the value does not fit in SMALLINT".to_owned())?,
        ),
        INT4_OID => out.put_i32(
            i32::try_from(value).map_err(|_| "the value does not fit in INTEGER".to_owned())?,
        ),
        INT8_OID => out.put_i64(
            i64::try_from(value).map_err(|_| "the value does not fit in BIGINT".to_owned())?,
        ),
        OID_OID => out
            .put_u32(u32::try_from(value).map_err(|_| "the value does not fit in OID".to_owned())?),
        NUMERIC_OID => write_numeric_signed(value, out)?,
        oid => return Err(type_mismatch("an integer", oid)),
    }
    Ok(())
}

/// Write an unsigned integer in whatever width the server inferred.
fn write_unsigned_integer(value: u128, ty: &Type, out: &mut BytesMut) -> Result<(), String> {
    match type_oid(ty) {
        INT2_OID => out.put_i16(
            i16::try_from(value).map_err(|_| "the value does not fit in SMALLINT".to_owned())?,
        ),
        INT4_OID => out.put_i32(
            i32::try_from(value).map_err(|_| "the value does not fit in INTEGER".to_owned())?,
        ),
        INT8_OID => out.put_i64(
            i64::try_from(value).map_err(|_| "the value does not fit in BIGINT".to_owned())?,
        ),
        OID_OID => out
            .put_u32(u32::try_from(value).map_err(|_| "the value does not fit in OID".to_owned())?),
        NUMERIC_OID => write_numeric_unsigned(value, out)?,
        oid => return Err(type_mismatch("an unsigned integer", oid)),
    }
    Ok(())
}

/// Write text, or the decimal digits when the inferred type is `NUMERIC`.
///
/// A `String` is normally bound to a text column. The numeric branch exists for array
/// elements and range bounds, where the element type is already known to be `NUMERIC`.
fn write_text(value: &str, ty: &Type, out: &mut BytesMut) -> Result<(), String> {
    if type_oid(ty) == NUMERIC_OID {
        write_numeric_text(value, out)?;
    } else {
        out.put_slice(value.as_bytes());
    }
    Ok(())
}

/// Encode decimal text into PostgreSQL's binary `NUMERIC` representation.
///
/// The wire format is a header of `ndigits`, `weight`, `sign` and `dscale`, followed by
/// base-10,000 digits, most significant first. `weight` is the exponent of the first digit,
/// and `dscale` is how many digits after the decimal point to display.
fn write_numeric_text(text: &str, out: &mut BytesMut) -> Result<(), String> {
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
        return Err(Error::encode(text, "the decimal is not a plain digit string").to_string());
    }

    // Each group is four decimal digits, aligned on the decimal point, so the integer part
    // is padded on the left and the fraction on the right.
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
        for _ in digits_in_group..4 {
            group *= 10;
        }
        groups.push(group);
    }

    let integer_groups = integer.len().div_ceil(4);
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
    let dscale =
        i16::try_from(fraction.len()).map_err(|_| "the decimal scale is too large".to_owned())?;

    out.put_i16(ndigits);
    out.put_i16(weight as i16);
    out.put_u16(sign);
    out.put_i16(dscale);
    for group in &groups[first..end] {
        out.put_u16(*group);
    }
    Ok(())
}

/// PostgreSQL's `NUMERIC` sign for a negative value; zero means positive.
const NUMERIC_SIGN_NEGATIVE: u16 = 0x4000;

/// The number of base-10,000 digits in the widest `NUMERIC` this driver sends.
///
/// `u128::MAX` has 39 decimal digits, which is ten groups of four.
const NUMERIC_GROUPS: usize = 10;

/// Write a signed integer as `NUMERIC(.., 0)` without allocating its decimal spelling.
fn write_numeric_signed(value: i128, out: &mut BytesMut) -> Result<(), String> {
    write_numeric_magnitude(value < 0, value.unsigned_abs(), out);
    Ok(())
}

/// Write an unsigned integer as `NUMERIC(.., 0)` without allocating its decimal spelling.
fn write_numeric_unsigned(value: u128, out: &mut BytesMut) -> Result<(), String> {
    write_numeric_magnitude(false, value, out);
    Ok(())
}

fn write_numeric_magnitude(negative: bool, magnitude: u128, out: &mut BytesMut) {
    let mut groups = [0u16; NUMERIC_GROUPS];
    let mut count = 0;
    let mut rest = magnitude;
    while rest != 0 {
        groups[count] = (rest % 10_000) as u16;
        rest /= 10_000;
        count += 1;
    }
    // The groups were collected least significant first. Low-order zero groups are not part
    // of the canonical form, and dropping them does not move `weight`, which counts from the
    // most significant digit.
    let mut first = 0;
    while first + 1 < count && groups[first] == 0 {
        first += 1;
    }
    let digits = &groups[first..count];

    let ndigits = digits.len() as i16;
    // A zero has no digits and no weight, which is the canonical form PostgreSQL writes.
    let weight = if count == 0 {
        0
    } else {
        (count as i16).saturating_sub(1)
    };
    let sign = if negative && ndigits != 0 {
        NUMERIC_SIGN_NEGATIVE
    } else {
        0
    };

    out.put_i16(ndigits);
    out.put_i16(weight);
    out.put_u16(sign);
    out.put_i16(0);
    for group in digits.iter().rev() {
        out.put_u16(*group);
    }
}

/// A bound array, encoded into PostgreSQL's binary array format.
///
/// The element's OID comes from the array type the server inferred, so an `INTEGER[]`
/// column carries `INT4` and its elements are written four bytes wide.
fn write_array(values: &[Value], ty: &Type, out: &mut BytesMut) -> Result<(), String> {
    let element = match base_type(ty).kind() {
        Kind::Array(element) => element,
        _ => return Err(type_mismatch("an array", type_oid(ty))),
    };

    let has_null = values.iter().any(Value::is_null);
    out.put_i32(1);
    out.put_i32(i32::from(has_null));
    out.put_u32(element.oid());
    out.put_i32(
        i32::try_from(values.len()).map_err(|_| "the array has too many elements".to_owned())?,
    );
    // PostgreSQL's arrays are one-based.
    out.put_i32(1);

    let mut payload = BytesMut::new();
    for value in values {
        if value.is_null() {
            out.put_i32(-1);
            continue;
        }
        payload.clear();
        encode(value, element, &mut payload)?;
        let length =
            i32::try_from(payload.len()).map_err(|_| "an element is too large".to_owned())?;
        out.put_i32(length);
        out.put_slice(&payload);
    }
    Ok(())
}

/// Write a range in PostgreSQL's binary range format.
///
/// `range_send` writes one flag byte and then, for each finite bound, the bound's own
/// encoding behind a four-byte length, exactly as an array element is written.
fn write_range(element: &Type, range: &Range<Value>, out: &mut BytesMut) -> Result<(), String> {
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
        out.put_u8(EMPTY);
        return Ok(());
    };

    let mut flags = 0_u8;
    match start {
        Bound::Included(_) => flags |= LOWER_INCLUDED,
        Bound::Excluded(_) => {}
        Bound::Unbounded => flags |= LOWER_INFINITE,
    }
    match end {
        Bound::Included(_) => flags |= UPPER_INCLUDED,
        Bound::Excluded(_) => {}
        Bound::Unbounded => flags |= UPPER_INFINITE,
    }
    out.put_u8(flags);

    for bound in [start, end] {
        let Some(value) = bound.value() else {
            continue;
        };
        let mut payload = BytesMut::new();
        encode(value, element, &mut payload)?;
        let length =
            i32::try_from(payload.len()).map_err(|_| "a range bound is too large".to_owned())?;
        out.put_i32(length);
        out.put_slice(&payload);
    }
    Ok(())
}

/// An `INET` or `CIDR`, split into the fields PostgreSQL's binary format needs.
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
    /// unspecified fallback keeps a hand-built value from panicking.
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

/// Write the five fields of PostgreSQL's network binary format.
fn write_network(value: &NetworkValue, out: &mut BytesMut) {
    // `PGSQL_AF_INET` is 2 and `PGSQL_AF_INET6` is 3; then the netmask, whether this is a
    // `cidr`, the address length, and the address.
    let (family, bytes): (u8, &[u8]) = match &value.address {
        core::net::IpAddr::V4(address) => (2, &address.octets()),
        core::net::IpAddr::V6(address) => (3, &address.octets()),
    };
    out.put_slice(&[
        family,
        value.prefix,
        u8::from(value.cidr),
        bytes.len() as u8,
    ]);
    out.put_slice(bytes);
}

/// PostgreSQL counts dates and timestamps from 2000-01-01, not from the Unix epoch.
fn postgres_epoch() -> chrono::NaiveDate {
    // 2000-01-01 is always representable, so the fallback is unreachable; returning the Unix
    // epoch instead of panicking keeps the workspace's no-panic rule.
    chrono::NaiveDate::from_ymd_opt(2000, 1, 1).unwrap_or_default()
}

/// A naive timestamp as whole microseconds since PostgreSQL's epoch.
fn micros_since_epoch(value: chrono::NaiveDateTime) -> Result<i64, String> {
    value
        .signed_duration_since(postgres_epoch().and_time(chrono::NaiveTime::MIN))
        .num_microseconds()
        .ok_or_else(|| "the timestamp is out of range".to_owned())
}

// ---------------------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------------------

/// A column read without interpreting its bytes.
///
/// `tokio_postgres` exposes decoding through `FromSql`, whose implementations check the
/// declared SQL type. Besu's `Decode` is about the Rust type, so this wrapper accepts every
/// SQL type and hands back the raw payload; [`Decode`] then decides what the bytes mean.
/// Wrapped in `Option`, it is also how `NULL` is distinguished without a public `is_null`.
struct RawBytes<'a>(&'a [u8]);

impl<'a> FromSql<'a> for RawBytes<'a> {
    fn from_sql(_: &Type, raw: &'a [u8]) -> Result<Self, Box<dyn StdError + Sync + Send>> {
        Ok(Self(raw))
    }

    fn accepts(_: &Type) -> bool {
        true
    }
}

/// One borrowed column, with the index that a decode error reports.
struct ColumnRef<'a> {
    index: usize,
    ty: &'a Type,
    bytes: &'a [u8],
}

impl ColumnRef<'_> {
    fn error(&self, reason: impl Into<String>) -> NativeError {
        decode_error(self.index, reason)
    }

    fn out_of_range<T>(&self) -> NativeError {
        self.error(format!(
            "value is out of range for `{}`",
            core::any::type_name::<T>()
        ))
    }
}

fn decode_error(index: usize, reason: impl Into<String>) -> NativeError {
    NativeError::Core(Error::decode(index, reason))
}

/// Take the next column, advancing the offset.
fn next_column<'a>(row: &'a NativeRow, offset: &mut usize) -> Result<ColumnRef<'a>, NativeError> {
    let index = *offset;
    *offset += 1;
    let ty = row
        .inner
        .columns()
        .get(index)
        .ok_or_else(|| decode_error(index, "the row has no column at this position"))?
        .type_();
    let raw = row
        .inner
        .try_get::<_, Option<RawBytes>>(index)
        .map_err(|err| decode_error(index, err.to_string()))?;
    let bytes = raw
        .ok_or_else(|| decode_error(index, "the column is NULL"))?
        .0;
    Ok(ColumnRef { index, ty, bytes })
}

fn read_i16(column: &ColumnRef<'_>) -> Result<i16, NativeError> {
    let bytes: [u8; 2] = column
        .bytes
        .try_into()
        .map_err(|_| column.error("malformed SMALLINT value"))?;
    Ok(i16::from_be_bytes(bytes))
}

fn read_i32(column: &ColumnRef<'_>) -> Result<i32, NativeError> {
    let bytes: [u8; 4] = column
        .bytes
        .try_into()
        .map_err(|_| column.error("malformed INTEGER value"))?;
    Ok(i32::from_be_bytes(bytes))
}

fn read_i64(column: &ColumnRef<'_>) -> Result<i64, NativeError> {
    let bytes: [u8; 8] = column
        .bytes
        .try_into()
        .map_err(|_| column.error("malformed BIGINT value"))?;
    Ok(i64::from_be_bytes(bytes))
}

fn parse_bool(column: &ColumnRef<'_>) -> Result<bool, NativeError> {
    if type_oid(column.ty) != BOOL_OID {
        return Err(not_a(column, "boolean"));
    }
    match column.bytes {
        [value] => Ok(*value != 0),
        _ => Err(column.error("malformed BOOLEAN value")),
    }
}

fn parse_i128(column: &ColumnRef<'_>) -> Result<i128, NativeError> {
    match type_oid(column.ty) {
        INT2_OID => Ok(i128::from(read_i16(column)?)),
        INT4_OID => Ok(i128::from(read_i32(column)?)),
        INT8_OID => Ok(i128::from(read_i64(column)?)),
        OID_OID => Ok(i128::from(u32::from_be_bytes(
            column
                .bytes
                .try_into()
                .map_err(|_| column.error("malformed OID value"))?,
        ))),
        NUMERIC_OID => {
            let (negative, magnitude) =
                decode_numeric_binary(column.bytes).map_err(|reason| column.error(reason))?;
            if negative {
                // `0 - magnitude` is representable even at `i128::MIN`.
                0i128
                    .checked_sub_unsigned(magnitude)
                    .ok_or_else(|| column.error("the NUMERIC value is too small for `i128`"))
            } else {
                i128::try_from(magnitude)
                    .map_err(|_| column.error("the NUMERIC value is too large for `i128`"))
            }
        }
        _ => Err(not_a(column, "integer")),
    }
}

fn parse_u128(column: &ColumnRef<'_>) -> Result<u128, NativeError> {
    match type_oid(column.ty) {
        NUMERIC_OID => {
            let (negative, magnitude) =
                decode_numeric_binary(column.bytes).map_err(|reason| column.error(reason))?;
            if negative {
                Err(column.error("a negative NUMERIC cannot be decoded as an unsigned integer"))
            } else {
                Ok(magnitude)
            }
        }
        _ => {
            let value = parse_i128(column)?;
            u128::try_from(value).map_err(|_| column.out_of_range::<u128>())
        }
    }
}

fn parse_f64(column: &ColumnRef<'_>) -> Result<f64, NativeError> {
    match type_oid(column.ty) {
        FLOAT4_OID => {
            let bytes: [u8; 4] = column
                .bytes
                .try_into()
                .map_err(|_| column.error("malformed REAL value"))?;
            Ok(f64::from(f32::from_be_bytes(bytes)))
        }
        FLOAT8_OID => {
            let bytes: [u8; 8] = column
                .bytes
                .try_into()
                .map_err(|_| column.error("malformed DOUBLE PRECISION value"))?;
            Ok(f64::from_be_bytes(bytes))
        }
        _ => Err(not_a(column, "floating-point number")),
    }
}

fn parse_f32(column: &ColumnRef<'_>) -> Result<f32, NativeError> {
    match type_oid(column.ty) {
        FLOAT4_OID => {
            let bytes: [u8; 4] = column
                .bytes
                .try_into()
                .map_err(|_| column.error("malformed REAL value"))?;
            Ok(f32::from_be_bytes(bytes))
        }
        FLOAT8_OID => Ok(parse_f64(column)? as f32),
        _ => Err(not_a(column, "floating-point number")),
    }
}

fn parse_text(column: &ColumnRef<'_>) -> Result<String, NativeError> {
    if type_oid(column.ty) == NUMERIC_OID {
        return decode_numeric_decimal(column.bytes).map_err(|reason| column.error(reason));
    }
    if !is_text_type(column.ty) {
        return Err(not_a(column, "text"));
    }
    core::str::from_utf8(column.bytes)
        .map(str::to_owned)
        .map_err(|_| column.error("the text is not valid UTF-8"))
}

fn is_text_type(ty: &Type) -> bool {
    matches!(
        type_oid(ty),
        CHAR_OID | NAME_OID | TEXT_OID | BPCHAR_OID | VARCHAR_OID | JSON_OID
    ) || matches!(base_type(ty).kind(), Kind::Enum(_))
}

fn parse_uuid(column: &ColumnRef<'_>) -> Result<uuid::Uuid, NativeError> {
    if type_oid(column.ty) != UUID_OID {
        return Err(not_a(column, "UUID"));
    }
    let bytes: [u8; 16] = column
        .bytes
        .try_into()
        .map_err(|_| column.error("malformed UUID value"))?;
    Ok(uuid::Uuid::from_bytes(bytes))
}

fn parse_date(column: &ColumnRef<'_>) -> Result<chrono::NaiveDate, NativeError> {
    if type_oid(column.ty) != DATE_OID {
        return Err(not_a(column, "DATE"));
    }
    let days = i64::from(read_i32(column)?);
    postgres_epoch()
        .checked_add_signed(chrono::Duration::days(days))
        .ok_or_else(|| column.error("the date is out of range"))
}

fn parse_time(column: &ColumnRef<'_>) -> Result<chrono::NaiveTime, NativeError> {
    if type_oid(column.ty) != TIME_OID {
        return Err(not_a(column, "TIME"));
    }
    let micros = read_i64(column)?;
    if !(0..86_400_000_000).contains(&micros) {
        return Err(column.error("the time is out of range"));
    }
    let seconds =
        u32::try_from(micros / 1_000_000).map_err(|_| column.error("the time is out of range"))?;
    let nanos = u32::try_from((micros % 1_000_000) * 1_000)
        .map_err(|_| column.error("the time is out of range"))?;
    chrono::NaiveTime::from_num_seconds_from_midnight_opt(seconds, nanos)
        .ok_or_else(|| column.error("the time is out of range"))
}

fn parse_naive_datetime(column: &ColumnRef<'_>) -> Result<chrono::NaiveDateTime, NativeError> {
    if !matches!(type_oid(column.ty), TIMESTAMP_OID | TIMESTAMPTZ_OID) {
        return Err(not_a(column, "TIMESTAMP"));
    }
    let micros = read_i64(column)?;
    postgres_epoch()
        .and_time(chrono::NaiveTime::MIN)
        .checked_add_signed(chrono::Duration::microseconds(micros))
        .ok_or_else(|| column.error("the timestamp is out of range"))
}

fn parse_utc_datetime(
    column: &ColumnRef<'_>,
) -> Result<chrono::DateTime<chrono::Utc>, NativeError> {
    Ok(parse_naive_datetime(column)?.and_utc())
}

fn parse_json(column: &ColumnRef<'_>) -> Result<serde_json::Value, NativeError> {
    let bytes = match type_oid(column.ty) {
        JSON_OID => column.bytes,
        // The binary JSONB form is a one-byte version followed by the document.
        JSONB_OID => match column.bytes.split_first() {
            Some((1, rest)) => rest,
            _ => return Err(column.error("malformed JSONB value")),
        },
        _ => return Err(not_a(column, "JSON")),
    };
    serde_json::from_slice(bytes).map_err(|err| column.error(format!("invalid JSON: {err}")))
}

fn parse_interval(column: &ColumnRef<'_>) -> Result<SqlInterval, NativeError> {
    if type_oid(column.ty) != INTERVAL_OID {
        return Err(not_a(column, "INTERVAL"));
    }
    decode_interval_binary(column.bytes).map_err(|reason| column.error(reason))
}

fn parse_network(column: &ColumnRef<'_>) -> Result<String, NativeError> {
    if !matches!(type_oid(column.ty), INET_OID | CIDR_OID) {
        return Err(not_a(column, "INET or CIDR"));
    }
    decode_network_binary(column.bytes).map_err(|reason| column.error(reason))
}

fn parse_bit_string(column: &ColumnRef<'_>) -> Result<BitString, NativeError> {
    if !matches!(type_oid(column.ty), BIT_OID | VARBIT_OID) {
        return Err(not_a(column, "BIT"));
    }
    let header = column
        .bytes
        .get(..4)
        .ok_or_else(|| column.error("malformed BIT value"))?;
    let length = i32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    let length = usize::try_from(length).map_err(|_| column.error("the bit length is negative"))?;
    BitString::from_bytes(&column.bytes[4..], length).map_err(|err| column.error(err.to_string()))
}

fn not_a(column: &ColumnRef<'_>, what: &str) -> NativeError {
    column.error(format!(
        "a `{}` value is not a {what}",
        base_type(column.ty).name()
    ))
}

macro_rules! impl_decode_integer {
    ($($ty:ty),* $(,)?) => {
        $(
            impl Decode<Native> for $ty {
                fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
                    let column = next_column(row, offset)?;
                    <$ty>::try_from(parse_i128(&column)?)
                        .map_err(|_| column.out_of_range::<$ty>())
                }
            }
        )*
    };
}

impl_decode_integer!(i8, i16, i32, i64, u8, u16, u32);

macro_rules! impl_decode_unsigned {
    ($($ty:ty),* $(,)?) => {
        $(
            impl Decode<Native> for $ty {
                fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
                    let column = next_column(row, offset)?;
                    <$ty>::try_from(parse_u128(&column)?)
                        .map_err(|_| column.out_of_range::<$ty>())
                }
            }
        )*
    };
}

impl_decode_unsigned!(u64, u128);

impl Decode<Native> for i128 {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let column = next_column(row, offset)?;
        parse_i128(&column)
    }
}

impl Decode<Native> for bool {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        parse_bool(&next_column(row, offset)?)
    }
}

impl Decode<Native> for f32 {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        parse_f32(&next_column(row, offset)?)
    }
}

impl Decode<Native> for f64 {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        parse_f64(&next_column(row, offset)?)
    }
}

impl Decode<Native> for String {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        parse_text(&next_column(row, offset)?)
    }
}

impl Decode<Native> for Vec<u8> {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let column = next_column(row, offset)?;
        if type_oid(column.ty) != BYTEA_OID {
            return Err(not_a(&column, "BYTEA"));
        }
        Ok(column.bytes.to_vec())
    }
}

impl Decode<Native> for uuid::Uuid {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        parse_uuid(&next_column(row, offset)?)
    }
}

impl Decode<Native> for chrono::NaiveDate {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        parse_date(&next_column(row, offset)?)
    }
}

impl Decode<Native> for chrono::NaiveTime {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        parse_time(&next_column(row, offset)?)
    }
}

impl Decode<Native> for chrono::NaiveDateTime {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        parse_naive_datetime(&next_column(row, offset)?)
    }
}

impl Decode<Native> for chrono::DateTime<chrono::Utc> {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        parse_utc_datetime(&next_column(row, offset)?)
    }
}

impl Decode<Native> for serde_json::Value {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        parse_json(&next_column(row, offset)?)
    }
}

impl Decode<Native> for SqlInterval {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        parse_interval(&next_column(row, offset)?)
    }
}

impl Decode<Native> for Inet {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let column = next_column(row, offset)?;
        Self::new(&parse_network(&column)?).map_err(|err| column.error(err.to_string()))
    }
}

impl Decode<Native> for Cidr {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let column = next_column(row, offset)?;
        Self::new(&parse_network(&column)?).map_err(|err| column.error(err.to_string()))
    }
}

impl Decode<Native> for BitString {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        parse_bit_string(&next_column(row, offset)?)
    }
}

impl Decode<Native> for Geometry {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let column = next_column(row, offset)?;
        Ok(Self::from_ewkb_unchecked(column.bytes.to_vec()))
    }
}

impl Decode<Native> for Vector {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let column = next_column(row, offset)?;
        Self::from_binary(column.bytes).map_err(|err| column.error(err.to_string()))
    }
}

// ---------------------------------------------------------------------------------------
// Ranges and arrays
// ---------------------------------------------------------------------------------------

/// A type that can be read out of one element of a PostgreSQL array or range.
trait ArrayElement: Sized {
    /// Decode one element from its binary form and the element's declared type.
    fn decode_element(index: usize, ty: &Type, bytes: &[u8]) -> Result<Self, NativeError>;
}

fn decode_range<T: ArrayElement>(column: &ColumnRef<'_>) -> Result<Range<T>, NativeError> {
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

    let element = match base_type(column.ty).kind() {
        Kind::Range(element) => element,
        _ => return Err(not_a(column, "range")),
    };

    let (flags, mut rest) = column
        .bytes
        .split_first()
        .ok_or_else(|| column.error("malformed range value"))?;
    if flags & ANY_NULL != 0 {
        return Err(column.error("a range with a NULL bound does not fit in `Range`"));
    }
    if flags & EMPTY != 0 {
        return Ok(Range::empty());
    }

    let mut decode_bound = |infinite: u8, included: u8| -> Result<Bound<T>, NativeError> {
        if flags & infinite != 0 {
            return Ok(Bound::Unbounded);
        }
        let length = rest
            .get(..4)
            .ok_or_else(|| column.error("malformed range value"))?;
        let length = i32::from_be_bytes([length[0], length[1], length[2], length[3]]);
        let length = usize::try_from(length).map_err(|_| column.error("malformed range value"))?;
        rest = rest
            .get(4..)
            .ok_or_else(|| column.error("malformed range value"))?;
        let (payload, tail) = rest
            .split_at_checked(length)
            .ok_or_else(|| column.error("malformed range value"))?;
        rest = tail;
        let value = T::decode_element(column.index, element, payload)?;
        Ok(if flags & included != 0 {
            Bound::Included(value)
        } else {
            Bound::Excluded(value)
        })
    };
    let start = decode_bound(LOWER_INFINITE, LOWER_INCLUDED)?;
    let end = decode_bound(UPPER_INFINITE, UPPER_INCLUDED)?;
    Ok(Range::new(start, end))
}

impl<T: ArrayElement> Decode<Native> for Range<T> {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        decode_range(&next_column(row, offset)?)
    }
}

fn decode_array<T: ArrayElement>(column: &ColumnRef<'_>) -> Result<Vec<T>, NativeError> {
    let element = match base_type(column.ty).kind() {
        Kind::Array(element) => element,
        _ => return Err(not_a(column, "array")),
    };

    let header = column
        .bytes
        .get(..12)
        .ok_or_else(|| column.error("malformed array value"))?;
    let dimensions = i32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    // An empty array is written with no dimensions at all, not one dimension of length zero.
    if dimensions == 0 {
        return Ok(Vec::new());
    }
    if dimensions != 1 {
        return Err(column.error(
            "only one-dimensional arrays decode into a `Vec`; an array of arrays needs `sql!`",
        ));
    }

    let dimension = column
        .bytes
        .get(12..20)
        .ok_or_else(|| column.error("malformed array value"))?;
    let count = i32::from_be_bytes([dimension[0], dimension[1], dimension[2], dimension[3]]);
    if count < 0 {
        return Err(column.error("malformed array value"));
    }

    let mut out = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
    let mut rest = &column.bytes[20..];
    while !rest.is_empty() {
        let length = rest
            .get(..4)
            .ok_or_else(|| column.error("malformed array value"))?;
        let length = i32::from_be_bytes([length[0], length[1], length[2], length[3]]);
        rest = &rest[4..];
        if length < 0 {
            return Err(
                column.error("the array contains a NULL element, which a `Vec<T>` cannot hold")
            );
        }
        let length = usize::try_from(length).map_err(|_| column.error("malformed array value"))?;
        let (payload, tail) = rest
            .split_at_checked(length)
            .ok_or_else(|| column.error("malformed array value"))?;
        out.push(T::decode_element(column.index, element, payload)?);
        rest = tail;
    }
    Ok(out)
}

impl<T: ArrayElement> Decode<Native> for Vec<T> {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        decode_array(&next_column(row, offset)?)
    }
}

macro_rules! impl_array_integer {
    ($($ty:ty),* $(,)?) => {
        $(
            impl ArrayElement for $ty {
                fn decode_element(
                    index: usize,
                    ty: &Type,
                    bytes: &[u8],
                ) -> Result<Self, NativeError> {
                    let column = ColumnRef { index, ty, bytes };
                    <$ty>::try_from(parse_i128(&column)?)
                        .map_err(|_| column.out_of_range::<$ty>())
                }
            }
        )*
    };
}

impl_array_integer!(i8, i16, i32, i64, u16, u32);

macro_rules! impl_array_unsigned {
    ($($ty:ty),* $(,)?) => {
        $(
            impl ArrayElement for $ty {
                fn decode_element(
                    index: usize,
                    ty: &Type,
                    bytes: &[u8],
                ) -> Result<Self, NativeError> {
                    let column = ColumnRef { index, ty, bytes };
                    <$ty>::try_from(parse_u128(&column)?)
                        .map_err(|_| column.out_of_range::<$ty>())
                }
            }
        )*
    };
}

impl_array_unsigned!(u64, u128);

macro_rules! impl_array_element {
    ($ty:ty => $parse:path) => {
        impl ArrayElement for $ty {
            fn decode_element(index: usize, ty: &Type, bytes: &[u8]) -> Result<Self, NativeError> {
                $parse(&ColumnRef { index, ty, bytes })
            }
        }
    };
}

impl_array_element!(bool => parse_bool);
impl_array_element!(f32 => parse_f32);
impl_array_element!(f64 => parse_f64);
impl_array_element!(String => parse_text);
impl_array_element!(uuid::Uuid => parse_uuid);
impl_array_element!(chrono::NaiveDate => parse_date);
impl_array_element!(chrono::NaiveTime => parse_time);
impl_array_element!(chrono::NaiveDateTime => parse_naive_datetime);
impl_array_element!(chrono::DateTime<chrono::Utc> => parse_utc_datetime);
impl_array_element!(serde_json::Value => parse_json);
impl_array_element!(SqlInterval => parse_interval);
impl_array_element!(BitString => parse_bit_string);

impl ArrayElement for Inet {
    fn decode_element(index: usize, ty: &Type, bytes: &[u8]) -> Result<Self, NativeError> {
        let column = ColumnRef { index, ty, bytes };
        Self::new(&parse_network(&column)?).map_err(|err| column.error(err.to_string()))
    }
}

impl ArrayElement for Cidr {
    fn decode_element(index: usize, ty: &Type, bytes: &[u8]) -> Result<Self, NativeError> {
        let column = ColumnRef { index, ty, bytes };
        Self::new(&parse_network(&column)?).map_err(|err| column.error(err.to_string()))
    }
}

impl<T: ArrayElement> ArrayElement for Range<T> {
    fn decode_element(index: usize, ty: &Type, bytes: &[u8]) -> Result<Self, NativeError> {
        decode_range(&ColumnRef { index, ty, bytes })
    }
}

// ---------------------------------------------------------------------------------------
// Binary parsers
// ---------------------------------------------------------------------------------------

/// Decode the binary wire format of an integral `NUMERIC` into its sign and magnitude.
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
    if (bytes.len() - 8) % 2 != 0 || usize::from(ndigits) != (bytes.len() - 8) / 2 {
        return Err("malformed NUMERIC value");
    }
    if ndigits == 0 {
        return Ok((negative, 0));
    }

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

/// Turn the binary `NUMERIC` wire format into its decimal text.
///
/// `dscale` is the number of digits the column displays after the point, so it is what makes
/// `1.90` come back as `1.90` rather than `1.9`.
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
    if (bytes.len() - 8) % 2 != 0 || usize::from(ndigits) != (bytes.len() - 8) / 2 {
        return Err("malformed NUMERIC value");
    }

    let mut groups: Vec<u16> = Vec::with_capacity(usize::from(ndigits));
    for position in 0..usize::from(ndigits) {
        let offset = 8 + position * 2;
        let group = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
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

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn numeric(text: &str) -> Vec<u8> {
        let mut out = BytesMut::new();
        write_numeric_text(text, &mut out).unwrap();
        out.to_vec()
    }

    #[test]
    fn a_decimal_round_trips_through_its_wire_format() {
        for text in ["0", "0.00", "1.23", "-1234567890.99", "9.90", "10000"] {
            assert_eq!(decode_numeric_decimal(&numeric(text)).unwrap(), text);
        }
        let (negative, magnitude) = decode_numeric_binary(&numeric("-1234")).unwrap();
        assert!(negative);
        assert_eq!(magnitude, 1234);
    }

    #[test]
    fn a_wide_integer_is_written_as_numeric() {
        let mut out = BytesMut::new();
        write_numeric_unsigned(u128::MAX, &mut out).unwrap();
        assert_eq!(decode_numeric_binary(&out).unwrap(), (false, u128::MAX));

        let mut out = BytesMut::new();
        write_numeric_signed(i128::MIN, &mut out).unwrap();
        let (negative, magnitude) = decode_numeric_binary(&out).unwrap();
        assert!(negative);
        assert_eq!(magnitude, i128::MIN.unsigned_abs());
    }

    #[test]
    fn a_range_is_written_with_its_flags_and_read_back() {
        fn decode(bytes: &[u8]) -> Result<Range<i32>, NativeError> {
            let ty = Type::INT4_RANGE;
            decode_range::<i32>(&ColumnRef {
                index: 0,
                ty: &ty,
                bytes,
            })
        }

        let mut out = BytesMut::new();
        write_range(
            &Type::INT4,
            &Range::new(
                Bound::Included(Value::Int(1)),
                Bound::Included(Value::Int(5)),
            ),
            &mut out,
        )
        .unwrap();
        assert_eq!(out[0], 0x06);
        assert_eq!(decode(&out).unwrap(), Range::closed(1, 5));

        let mut out = BytesMut::new();
        write_range(&Type::INT4, &Range::empty(), &mut out).unwrap();
        assert_eq!(out.to_vec(), [0x01]);
        assert_eq!(decode(&out).unwrap(), Range::empty());
    }

    #[test]
    fn a_network_value_round_trips() {
        let value = NetworkValue::parse("2001:db8::1/64", false);
        let mut out = BytesMut::new();
        write_network(&value, &mut out);
        assert_eq!(
            decode_network_binary(&out).unwrap(),
            "2001:db8::1/64".to_owned()
        );
    }

    #[test]
    fn a_malformed_numeric_or_network_does_not_panic() {
        assert!(decode_numeric_binary(&[]).is_err());
        assert!(decode_numeric_binary(&[0, 1, 0, 0, 0, 0, 0, 0]).is_err());
        assert!(decode_numeric_decimal(&[]).is_err());
        assert!(decode_network_binary(&[]).is_err());
        assert!(decode_network_binary(&[2, 24, 0, 4, 1]).is_err());
    }
}
