//! A MySQL driver backed by [`mysql_async`], behind the `native` feature.
//!
//! The pool is a [`mysql_async::Pool`]: at most ten connections, opened lazily and recycled
//! when they are returned. A query checks one out, prepares its statement (the connection
//! caches it by SQL text), runs it in the binary protocol and gives the connection back.
//!
//! # TLS
//!
//! `mysql_async` is compiled without a TLS feature, so connections are unencrypted. TLS is
//! available in `mysql_async` as the `rustls-tls` (with a provider such as `ring`) or
//! `native-tls-tls` feature plus `SslOpts` on `Opts`; exposing it here would mean a
//! constructor that accepts those options, which is an API decision rather than a dependency
//! one. A URL that asks for TLS (`require_ssl=true`) hits `mysql_async`'s own guard, which
//! panics with a message naming the features to enable; it never silently goes unencrypted.
//! An application that enables a `mysql_async` TLS feature itself can still pass a pool built
//! with `SslOpts` to [`Native::from_pool`].
//!
//! # Transactions
//!
//! A transaction is a [`mysql_async::Transaction`], which owns one pooled connection until it
//! is committed or rolled back. Nested `tx.transaction(..)` calls are `SAVEPOINT` statements
//! issued by [`besu::Database`], exactly as they are for every other driver.
//!
//! # Streaming
//!
//! [`Driver::query_stream`] forwards rows from the server over a bounded channel, one row at a
//! time, so the pooled connection stays checked out until the stream is dropped and memory
//! stays bounded. Inside a transaction the stream borrows the transaction's connection instead
//! and needs no task.
//!
//! # The type mapping
//!
//! MySQL's binary protocol already makes the numeric, temporal and blob distinctions, so most
//! values arrive as [`mysql_async::Value`] variants rather than as raw bytes. `DECIMAL`, `SET`,
//! `ENUM`, `JSON`, `BIT` and spatial values arrive as byte strings, and the column's declared
//! type says which interpretation applies. The decimal digit parsers and the `BIT`/geometry
//! handling are the same rules as the sqlx driver: exact decimal text for `String`, truncation
//! toward zero for a 128-bit integer, and the significant bits for a `BIT(n)` column.

use core::{
    future::poll_fn,
    pin::Pin,
    task::{Context, Poll},
};

use besu_core::{
    BitString, Cidr, Decode, Driver, Error, Geometry, Inet, Range, RowStream, Sql, SqlInterval,
    Stream, TransactionDriver, Value, Vector,
};
use chrono::{Datelike as _, Timelike as _};
use futures_channel::mpsc;
use mysql_async::{
    Opts, OptsBuilder, Pool, PoolConstraints, Row, TxOpts, Value as MySqlValue, consts::ColumnType,
    prelude::Queryable as _,
};

use crate::MySql;

// ---------------------------------------------------------------------------------------
// Errors and output
// ---------------------------------------------------------------------------------------

/// Everything the native MySQL driver can fail with.
///
/// A statement rejected by the server, a connection that could not be opened and a value that
/// cannot be encoded or decoded all arrive here, so a caller has one `?` type.
#[derive(Debug)]
#[non_exhaustive]
pub enum NativeError {
    /// The server rejected a statement, or the connection failed.
    MySql(mysql_async::Error),
    /// A value could not be encoded, or a column could not be decoded.
    Core(Error),
}

impl core::fmt::Display for NativeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MySql(err) => write!(f, "{err}"),
            Self::Core(err) => write!(f, "{err}"),
        }
    }
}

impl core::error::Error for NativeError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::MySql(err) => Some(err),
            Self::Core(err) => Some(err),
        }
    }
}

impl From<mysql_async::Error> for NativeError {
    fn from(err: mysql_async::Error) -> Self {
        Self::MySql(err)
    }
}

impl From<mysql_async::UrlError> for NativeError {
    fn from(err: mysql_async::UrlError) -> Self {
        // A URL is a connection failure, and `mysql_async::Error` is the one type the driver
        // exposes for it, so the error keeps its shape instead of being flattened to text.
        Self::MySql(mysql_async::Error::Url(err))
    }
}

impl From<Error> for NativeError {
    fn from(err: Error) -> Self {
        Self::Core(err)
    }
}

/// What a statement that returns no rows reported.
///
/// MySQL reports both the number of changed rows and the id allocated by the last
/// `AUTO_INCREMENT` column, which is how a write gets its primary key without `RETURNING`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NativeOutput {
    rows_affected: u64,
    last_insert_id: Option<u64>,
}

impl NativeOutput {
    /// The number of rows the statement changed.
    pub const fn rows_affected(&self) -> u64 {
        self.rows_affected
    }

    /// The id the last `AUTO_INCREMENT` column allocated, if the statement allocated one.
    pub const fn last_insert_id(&self) -> Option<u64> {
        self.last_insert_id
    }
}

/// One row of a native MySQL result set.
///
/// The row owns its values; the column names are shared with every row of the result set
/// rather than copied into each one.
#[derive(Clone, Debug)]
pub struct NativeRow {
    inner: Row,
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
        matches!(self.inner.as_ref(index), None | Some(MySqlValue::NULL))
    }

    /// The names of the row's columns, in decode order.
    pub fn column_names(&self) -> Vec<String> {
        self.inner
            .columns_ref()
            .iter()
            .map(|column| column.name_str().into_owned())
            .collect()
    }

    /// The MySQL type name of the column at `index`, when it is known.
    ///
    /// This is the name for diagnostics, not a `mysql_async` type: it lets a caller print a
    /// result set's shape without depending on the driver's own crate.
    pub fn column_type(&self, index: usize) -> Option<&'static str> {
        self.mysql_type(index).map(column_type_name)
    }

    /// The MySQL type of a column, for the decoders that need to distinguish a `DECIMAL` or a
    /// `BIT` from an ordinary byte string.
    fn mysql_type(&self, index: usize) -> Option<ColumnType> {
        self.inner
            .columns_ref()
            .get(index)
            .map(|column| column.column_type())
    }
}

// ---------------------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------------------

/// A MySQL driver backed by [`mysql_async`] and its connection pool.
///
/// The constructors take a MySQL URL, are async because a connection is checked eagerly, and
/// require no TLS configuration.
#[derive(Clone)]
pub struct Native {
    pool: Pool,
}

impl core::fmt::Debug for Native {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // `mysql_async::Pool`'s own `Debug` prints its `Opts`, which contain the password.
        // A driver is logged by name and never by connection string.
        f.debug_struct("Native").finish_non_exhaustive()
    }
}

impl Native {
    /// Open a connection pool from a MySQL URL such as
    /// `mysql://user:password@localhost:3306/app`.
    ///
    /// The session is pinned to `utf8mb4` and `+00:00`, because [`chrono`] timestamps are
    /// encoded as UTC and a non-UTF-8 connection would mangle text.
    pub async fn new(url: &str) -> Result<Self, NativeError> {
        let opts = Opts::from_url(url)?;
        // `setup`, not `init`: it runs on a new connection and again whenever a connection is
        // reset. A caller who builds their own pool with reset enabled would otherwise lose
        // the pinned session after the first checkout.
        let builder = OptsBuilder::from_opts(opts).setup(vec![
            "SET NAMES utf8mb4".to_owned(),
            "SET time_zone = '+00:00'".to_owned(),
        ]);
        Self::with_opts(Opts::from(builder)).await
    }

    /// Open a connection pool from a MySQL URL, leaving every connection option exactly as
    /// the URL spells it.
    ///
    /// One connection is opened before returning, so an unreachable server is an error from
    /// the constructor rather than from the first query. The pool keeps its own ceiling of ten
    /// connections, which is the same maximum as the sqlx driver.
    pub async fn connect(url: &str) -> Result<Self, NativeError> {
        let opts = Opts::from_url(url)?;
        Self::with_opts(opts).await
    }

    async fn with_opts(opts: Opts) -> Result<Self, NativeError> {
        // The same maximum as the sqlx driver. A minimum of zero keeps connections lazy, so
        // constructing a database does not open ten sockets.
        let constraints = PoolConstraints::new(0, 10).unwrap_or_default();
        // Connections are not `COM_RESET_CONNECTION`ed on return, matching sqlx: a reset
        // would clear the prepared-statement cache and the pinned session on every checkout.
        // The pool still rolls back a transaction and drains a pending result before it
        // reuses a connection.
        let pool_opts = opts
            .pool_opts()
            .clone()
            .with_constraints(constraints)
            .with_reset_connection(false);
        let builder = OptsBuilder::from_opts(opts).pool_opts(pool_opts);
        let pool = Pool::new(Opts::from(builder));

        // Check the server is there now, so a typo in a URL is not reported by whichever
        // query happens to run first.
        let conn = pool.get_conn().await?;
        drop(conn);

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
    type Dialect = MySql;
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
        let column = ColumnRef {
            index,
            value: row
                .inner
                .as_ref(index)
                .ok_or_else(|| decode_error(index, "the row has no column at this position"))?,
            ty: row.mysql_type(index),
        };
        parse_text(&column)
    }

    async fn query(&self, sql: &Sql<MySql>) -> Result<Vec<Self::Row>, Self::Error> {
        let params = encode(sql)?;
        let mut conn = self.pool.get_conn().await?;
        let rows = conn.exec::<Row, _, _>(sql.sql(), params).await?;
        Ok(rows.into_iter().map(|inner| NativeRow { inner }).collect())
    }

    async fn execute(&self, sql: &Sql<MySql>) -> Result<Self::Output, Self::Error> {
        let params = encode(sql)?;
        let mut conn = self.pool.get_conn().await?;
        let result = conn.exec_iter(sql.sql(), params).await?;
        let output = NativeOutput {
            rows_affected: result.affected_rows(),
            last_insert_id: result.last_insert_id(),
        };
        result.drop_result().await?;
        Ok(output)
    }

    async fn query_stream<'a>(
        &'a self,
        sql: &'a Sql<MySql>,
    ) -> Result<RowStream<'a, Self>, Self::Error> {
        let params = encode(sql)?;
        let statement = sql.sql().to_owned();
        // Check the connection out here so a pool failure is reported by this call rather
        // than by the first item of the stream.
        let mut conn = self.pool.get_conn().await?;

        let (mut sender, receiver) = mpsc::channel(1);
        tokio::spawn(async move {
            match conn
                .exec_stream::<Row, _, _>(statement.as_str(), params)
                .await
            {
                Ok(rows) => {
                    let mut rows = Box::pin(rows);
                    while let Some(item) = poll_fn(|cx| rows.as_mut().poll_next(cx)).await {
                        if !send(&mut sender, item).await {
                            break;
                        }
                    }
                }
                Err(err) => {
                    // A receiver that has already gone is not an error worth reporting.
                    let _ = send(&mut sender, Err(err)).await;
                }
            }
        });

        Ok(Box::pin(NativeRowStream { rows: receiver }) as RowStream<'a, Self>)
    }
}

/// Send one row, waiting for room in the bounded channel.
///
/// `futures_channel`'s own `send` comes from `SinkExt`, which would add `futures-util` to the
/// dependency graph for one call; the bounded sender's inherent `poll_ready`/`start_send` pair
/// is the same future with nothing extra.
async fn send<T>(sender: &mut mpsc::Sender<T>, item: T) -> bool {
    if poll_fn(|cx| sender.poll_ready(cx)).await.is_err() {
        return false;
    }
    sender.start_send(item).is_ok()
}

/// A stream that owns a connection's row stream and maps each row into a [`NativeRow`].
///
/// The connection is owned by whatever produced `rows`: the spawned task for a pool query, or
/// the transaction for [`TransactionDriver::txn_query_stream`]. Dropping the stream drops that
/// owner, which returns the connection to the pool or ends the task.
struct NativeRowStream<S> {
    rows: S,
}

impl<S> Stream for NativeRowStream<S>
where
    S: Stream<Item = mysql_async::Result<Row>> + Unpin,
{
    type Item = Result<NativeRow, NativeError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.rows).poll_next(cx).map(|item| {
            item.map(|row| {
                row.map(|inner| NativeRow { inner })
                    .map_err(NativeError::MySql)
            })
        })
    }
}

// ---------------------------------------------------------------------------------------
// Transactions
// ---------------------------------------------------------------------------------------

/// A transaction on one pooled connection.
///
/// This is `mysql_async`'s own handle: dropping it without committing marks the connection as
/// requiring a rollback, and the pool's recycler rolls it back before handing the connection to
/// anyone else.
pub type NativeTransaction = mysql_async::Transaction<'static>;

impl TransactionDriver for Native {
    type Transaction<'a> = NativeTransaction;

    async fn begin<'a>(&'a self) -> Result<Self::Transaction<'a>, Self::Error> {
        Ok(self.pool.start_transaction(TxOpts::default()).await?)
    }

    async fn commit<'a>(tx: Self::Transaction<'a>) -> Result<(), Self::Error> {
        Ok(tx.commit().await?)
    }

    async fn rollback<'a>(tx: Self::Transaction<'a>) -> Result<(), Self::Error> {
        Ok(tx.rollback().await?)
    }

    async fn txn_query<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &Sql<MySql>,
    ) -> Result<Vec<Self::Row>, Self::Error> {
        let params = encode(sql)?;
        let rows = tx.exec::<Row, _, _>(sql.sql(), params).await?;
        Ok(rows.into_iter().map(|inner| NativeRow { inner }).collect())
    }

    async fn txn_execute<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &Sql<MySql>,
    ) -> Result<Self::Output, Self::Error> {
        let params = encode(sql)?;
        let result = tx.exec_iter(sql.sql(), params).await?;
        let output = NativeOutput {
            rows_affected: result.affected_rows(),
            last_insert_id: result.last_insert_id(),
        };
        result.drop_result().await?;
        Ok(output)
    }

    async fn txn_query_stream<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &'c Sql<MySql>,
    ) -> Result<RowStream<'c, Self>, Self::Error> {
        let params = encode(sql)?;
        // `stream_and_drop` takes ownership of the query result while still borrowing the
        // transaction's connection, so the returned stream has exactly the transaction's
        // lifetime and nothing is collected.
        let result = tx.exec_iter(sql.sql(), params).await?;
        let rows = result.stream_and_drop::<Row>().await?.ok_or_else(|| {
            NativeError::Core(Error::invalid_query(
                "the statement produced no result set to stream",
            ))
        })?;
        Ok(Box::pin(NativeRowStream {
            rows: Box::pin(rows),
        }) as RowStream<'c, Self>)
    }
}

// ---------------------------------------------------------------------------------------
// `SchemaDriver`
// ---------------------------------------------------------------------------------------

impl besu::SchemaDriver for Native {
    type Ddl = crate::MySqlDdl;

    async fn introspect(&self) -> Result<besu::schema::Schema, Self::Error> {
        let db = besu::Database::new(self.clone());
        crate::introspect::introspect(&db).await
    }

    /// Run every schema statement on one connection.
    ///
    /// `push` is not transactional — MySQL commits DDL implicitly — but it is one logical
    /// change, so a statement that depends on a setting another statement made sees the same
    /// session. The text protocol is used because a schema statement is not a cached query
    /// shape worth preparing.
    async fn execute_statements(&self, statements: &[String]) -> Result<(), Self::Error> {
        let mut conn = self.pool.get_conn().await?;
        for statement in statements {
            conn.query_drop(statement.as_str()).await?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------------------

/// Encode a statement's parameters as `mysql_async` values.
///
/// Each `Value` variant maps to exactly one `mysql_async::Value` variant, whatever its
/// magnitude. The two 128-bit types and `Decimal` are always sent as decimal text, never as a
/// narrower integer when they happen to fit, so a cached prepared statement sees one parameter
/// type per column across calls.
fn encode(sql: &Sql<MySql>) -> Result<Vec<MySqlValue>, NativeError> {
    sql.args()
        .iter()
        .map(|value| {
            Ok(match value {
                Value::Null => MySqlValue::NULL,
                // MySQL's `BOOLEAN` is `TINYINT(1)`, so a bool is a small integer.
                Value::Bool(value) => MySqlValue::Int(i64::from(*value)),
                Value::Int(value) => MySqlValue::Int(*value),
                Value::UInt(value) => MySqlValue::UInt(*value),
                // `DECIMAL(39,0)` is the column, and decimal text is what it accepts.
                Value::UInt128(value) => MySqlValue::Bytes(value.to_string().into_bytes()),
                Value::Int128(value) => MySqlValue::Bytes(value.to_string().into_bytes()),
                Value::Float(value) => MySqlValue::Double(*value),
                Value::Text(value) => MySqlValue::Bytes(value.as_bytes().to_vec()),
                // MySQL coerces a text parameter to the column's type, so a label binds
                // exactly like text.
                Value::EnumLabel(value) => MySqlValue::Bytes(value.as_bytes().to_vec()),
                // The exact decimal text goes over the wire unchanged and the server rounds it
                // to the column's scale; building a decimal type would copy the digits.
                Value::Decimal(value) => MySqlValue::Bytes(value.as_bytes().to_vec()),
                Value::Bytes(value) => MySqlValue::Bytes(value.clone()),
                Value::Uuid(value) => MySqlValue::Bytes(value.as_bytes().to_vec()),
                Value::Date(date) => mysql_date(*date, None, value)?,
                Value::Time(time) => MySqlValue::Time(
                    false,
                    0,
                    u8::try_from(time.hour()).unwrap_or(0),
                    u8::try_from(time.minute()).unwrap_or(0),
                    u8::try_from(time.second()).unwrap_or(0),
                    time.nanosecond() / 1_000,
                ),
                Value::Timestamp(stamp) => mysql_date(stamp.date(), Some(*stamp), value)?,
                Value::TimestampTz(stamp) => {
                    let stamp = stamp.naive_utc();
                    mysql_date(stamp.date(), Some(stamp), value)?
                }
                // MySQL has no interval type, so the column is `TEXT` and the canonical text
                // is exactly what is stored and read back.
                Value::Interval(value) => MySqlValue::Bytes(value.to_string().into_bytes()),
                // MySQL has no address type either; `TEXT` holds the canonical spelling.
                Value::Inet(value) => MySqlValue::Bytes(value.as_str().as_bytes().to_vec()),
                Value::Cidr(value) => MySqlValue::Bytes(value.as_str().as_bytes().to_vec()),
                // A `BIT(n)` column is a big-endian byte string, most significant bit first.
                Value::Bit(value) => MySqlValue::Bytes(value.to_bytes()),
                // MySQL has no range type, so the column is `TEXT` and the canonical
                // `[a,b)` spelling is exactly what is stored.
                Value::Range { range, .. } => {
                    MySqlValue::Bytes(Value::range_text(range).into_bytes())
                }
                // A spatial column accepts MySQL's internal format: the four-byte SRID then
                // the WKB. The value itself is EWKB, so it is converted rather than re-encoded.
                Value::Geometry(value) => MySqlValue::Bytes(value.to_mysql_bytes()),
                // MySQL 8.4 has no vector type; the `[1,2,3]` text is the storage.
                Value::Vector(value) => MySqlValue::Bytes(value.to_string().into_bytes()),
                Value::Json(value) => {
                    let text = std::str::from_utf8(value).unwrap_or("null");
                    let text = if text.is_empty() { "null" } else { text };
                    MySqlValue::Bytes(text.as_bytes().to_vec())
                }
                // MySQL has no array type. Binding `NULL` instead would turn a programming
                // mistake into a missing value, so the statement fails before it is sent.
                Value::Array { .. } => {
                    return Err(NativeError::Core(Error::encode(
                        value.display(),
                        "`mysql` has no array type; store the elements as JSON instead",
                    )));
                }
                // `Value` is `#[non_exhaustive]`: a future variant must fail loudly rather
                // than silently bind `NULL`.
                _ => {
                    return Err(NativeError::Core(Error::encode(
                        value.display(),
                        "this driver does not know how to bind this value",
                    )));
                }
            })
        })
        .collect()
}

/// Build the seven-field date value `mysql_async` serialises.
///
/// `mysql_async` needs the parts separately, and a timestamp with a zero time serialises to
/// the four-byte form, which the server reads back as midnight. The year is checked because
/// MySQL stores four digits.
fn mysql_date(
    date: chrono::NaiveDate,
    time: Option<chrono::NaiveDateTime>,
    displayed: &Value,
) -> Result<MySqlValue, NativeError> {
    let year = u16::try_from(date.year()).map_err(|_| {
        NativeError::Core(Error::encode(
            displayed.display(),
            "a MySQL date is limited to four-digit years",
        ))
    })?;
    let (hour, minute, second, micros) = match time {
        None => (0, 0, 0, 0),
        Some(value) => (
            u8::try_from(value.hour()).unwrap_or(0),
            u8::try_from(value.minute()).unwrap_or(0),
            u8::try_from(value.second()).unwrap_or(0),
            value.nanosecond() / 1_000,
        ),
    };
    Ok(MySqlValue::Date(
        year,
        u8::try_from(date.month()).unwrap_or(1),
        u8::try_from(date.day()).unwrap_or(1),
        hour,
        minute,
        second,
        micros,
    ))
}

// ---------------------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------------------

fn decode_error(index: usize, reason: impl Into<String>) -> NativeError {
    NativeError::Core(Error::decode(index, reason))
}

/// One value, with the index a decode error reports and the column's declared type.
struct ColumnRef<'a> {
    index: usize,
    value: &'a MySqlValue,
    ty: Option<ColumnType>,
}

impl ColumnRef<'_> {
    fn error(&self, reason: impl Into<String>) -> NativeError {
        decode_error(self.index, reason)
    }

    fn not_a(&self, what: &str) -> NativeError {
        self.error(format!(
            "a `{}` value is not {what}",
            self.ty.map(column_type_name).unwrap_or("unknown MySQL")
        ))
    }

    fn is_decimal(&self) -> bool {
        matches!(
            self.ty,
            Some(ColumnType::MYSQL_TYPE_DECIMAL | ColumnType::MYSQL_TYPE_NEWDECIMAL)
        )
    }

    fn bytes(&self) -> Result<&[u8], NativeError> {
        match self.value {
            MySqlValue::Bytes(bytes) => Ok(bytes),
            _ => Err(self.not_a("binary")),
        }
    }
}

/// Take the next column, advancing the offset.
fn next_column<'a>(row: &'a NativeRow, offset: &mut usize) -> Result<ColumnRef<'a>, NativeError> {
    let index = *offset;
    *offset += 1;
    let value = row
        .inner
        .as_ref(index)
        .ok_or_else(|| decode_error(index, "the row has no column at this position"))?;
    if matches!(value, MySqlValue::NULL) {
        return Err(decode_error(index, "the column is NULL"));
    }
    Ok(ColumnRef {
        index,
        value,
        ty: row.mysql_type(index),
    })
}

/// A name for a MySQL type, only used in error messages.
fn column_type_name(ty: ColumnType) -> &'static str {
    use ColumnType as C;
    match ty {
        C::MYSQL_TYPE_DECIMAL | C::MYSQL_TYPE_NEWDECIMAL => "DECIMAL",
        C::MYSQL_TYPE_TINY => "TINYINT",
        C::MYSQL_TYPE_SHORT => "SMALLINT",
        C::MYSQL_TYPE_LONG | C::MYSQL_TYPE_INT24 => "INT",
        C::MYSQL_TYPE_LONGLONG => "BIGINT",
        C::MYSQL_TYPE_FLOAT => "FLOAT",
        C::MYSQL_TYPE_DOUBLE => "DOUBLE",
        C::MYSQL_TYPE_NULL => "NULL",
        C::MYSQL_TYPE_TIMESTAMP | C::MYSQL_TYPE_TIMESTAMP2 => "TIMESTAMP",
        C::MYSQL_TYPE_DATE | C::MYSQL_TYPE_NEWDATE => "DATE",
        C::MYSQL_TYPE_TIME | C::MYSQL_TYPE_TIME2 => "TIME",
        C::MYSQL_TYPE_DATETIME | C::MYSQL_TYPE_DATETIME2 => "DATETIME",
        C::MYSQL_TYPE_YEAR => "YEAR",
        C::MYSQL_TYPE_VARCHAR => "VARCHAR",
        C::MYSQL_TYPE_VAR_STRING => "VAR_STRING",
        C::MYSQL_TYPE_STRING => "CHAR",
        C::MYSQL_TYPE_BIT => "BIT",
        C::MYSQL_TYPE_JSON => "JSON",
        C::MYSQL_TYPE_ENUM => "ENUM",
        C::MYSQL_TYPE_SET => "SET",
        C::MYSQL_TYPE_TINY_BLOB
        | C::MYSQL_TYPE_MEDIUM_BLOB
        | C::MYSQL_TYPE_LONG_BLOB
        | C::MYSQL_TYPE_BLOB => "BLOB",
        C::MYSQL_TYPE_GEOMETRY => "GEOMETRY",
        C::MYSQL_TYPE_VECTOR => "VECTOR",
        _ => "unknown MySQL",
    }
}

fn parse_bool(column: &ColumnRef<'_>) -> Result<bool, NativeError> {
    match column.value {
        MySqlValue::Int(value) => Ok(*value != 0),
        MySqlValue::UInt(value) => Ok(*value != 0),
        MySqlValue::Bytes(bytes) if bytes.as_slice() == b"0" => Ok(false),
        MySqlValue::Bytes(bytes) if bytes.as_slice() == b"1" => Ok(true),
        _ => Err(column.not_a("a boolean")),
    }
}

/// Decode an integer column as `i64`.
///
/// An unsigned column arrives as [`MySqlValue::UInt`], and a value above `i64::MAX` is an
/// error rather than a wrapped negative number.
fn parse_i64(column: &ColumnRef<'_>) -> Result<i64, NativeError> {
    match column.value {
        MySqlValue::Int(value) => Ok(*value),
        MySqlValue::UInt(value) => i64::try_from(*value)
            .map_err(|_| column.error("a MySQL unsigned integer is larger than i64::MAX")),
        _ => Err(column.not_a("an integer")),
    }
}

fn parse_u64(column: &ColumnRef<'_>) -> Result<u64, NativeError> {
    match column.value {
        MySqlValue::UInt(value) => Ok(*value),
        MySqlValue::Int(value) => u64::try_from(*value)
            .map_err(|_| column.error("a negative MySQL integer cannot be decoded as u64")),
        _ => Err(column.not_a("an integer")),
    }
}

/// Decode decimal text into the digits of its integer part.
///
/// MySQL always sends a `DECIMAL` as text with the column's own scale. A scaled value is
/// truncated toward zero, which is what the sqlx driver's `BigDecimal` conversion does; only
/// the sign and digits are needed here.
fn decimal_integer(text: &str) -> Option<(bool, &str)> {
    let text = text.trim();
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (integer, fraction) = match rest.split_once('.') {
        Some((integer, fraction)) => (integer, fraction),
        None => (rest, ""),
    };
    if (integer.is_empty() && fraction.is_empty())
        || !integer.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Some((negative, integer))
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

fn parse_f64(column: &ColumnRef<'_>) -> Result<f64, NativeError> {
    match column.value {
        MySqlValue::Double(value) => Ok(*value),
        MySqlValue::Float(value) => Ok(f64::from(*value)),
        _ => Err(column.not_a("a floating-point number")),
    }
}

fn parse_text(column: &ColumnRef<'_>) -> Result<String, NativeError> {
    match column.value {
        MySqlValue::Bytes(bytes) => String::from_utf8(bytes.clone())
            .map_err(|_| column.error("the value is not valid UTF-8")),
        _ => Err(column.not_a("text")),
    }
}

fn parse_uuid(column: &ColumnRef<'_>) -> Result<uuid::Uuid, NativeError> {
    let bytes = column.bytes()?;
    let bytes: [u8; 16] = bytes
        .try_into()
        .map_err(|_| column.error("a MySQL UUID must be 16 bytes"))?;
    Ok(uuid::Uuid::from_bytes(bytes))
}

fn parse_date(column: &ColumnRef<'_>) -> Result<chrono::NaiveDate, NativeError> {
    match column.value {
        MySqlValue::Date(year, month, day, _, _, _, _) => {
            chrono::NaiveDate::from_ymd_opt(i32::from(*year), u32::from(*month), u32::from(*day))
                .ok_or_else(|| column.error("the server returned an invalid date"))
        }
        _ => Err(column.not_a("a date")),
    }
}

fn parse_time(column: &ColumnRef<'_>) -> Result<chrono::NaiveTime, NativeError> {
    match column.value {
        MySqlValue::Time(negative, days, hours, minutes, seconds, micros) => {
            if *negative || *days != 0 {
                return Err(column.error(
                    "a MySQL TIME outside 00:00:00..24:00:00 cannot be decoded as NaiveTime",
                ));
            }
            chrono::NaiveTime::from_hms_micro_opt(
                u32::from(*hours),
                u32::from(*minutes),
                u32::from(*seconds),
                *micros,
            )
            .ok_or_else(|| column.error("the server returned an invalid time"))
        }
        _ => Err(column.not_a("a time")),
    }
}

fn parse_naive_datetime(column: &ColumnRef<'_>) -> Result<chrono::NaiveDateTime, NativeError> {
    match column.value {
        MySqlValue::Date(year, month, day, hour, minute, second, micros) => {
            let date = chrono::NaiveDate::from_ymd_opt(
                i32::from(*year),
                u32::from(*month),
                u32::from(*day),
            )
            .ok_or_else(|| column.error("the server returned an invalid date"))?;
            date.and_hms_micro_opt(
                u32::from(*hour),
                u32::from(*minute),
                u32::from(*second),
                *micros,
            )
            .ok_or_else(|| column.error("the server returned an invalid time"))
        }
        _ => Err(column.not_a("a timestamp")),
    }
}

fn parse_json(column: &ColumnRef<'_>) -> Result<serde_json::Value, NativeError> {
    let bytes = column.bytes()?;
    serde_json::from_slice(bytes).map_err(|err| column.error(format!("invalid JSON: {err}")))
}

fn parse_bit_string(column: &ColumnRef<'_>) -> Result<BitString, NativeError> {
    let bytes = column.bytes()?;
    let bits = BitString::from_bytes(bytes, bytes.len() * 8)
        .map_err(|err| column.error(err.to_string()))?;

    // `BIT(5)` holding `b'101'` arrives as the single byte `0b00000101`: MySQL keeps the value
    // as an integer, so the width is not on the wire. The significant bits are therefore all
    // that can be reported, and the leading zero padding the server added is dropped. A value
    // that is all zero bits arrives as `0`.
    let significant = bits.as_str().trim_start_matches('0');
    BitString::new(if significant.is_empty() {
        "0"
    } else {
        significant
    })
    .map_err(|err| column.error(err.to_string()))
}

// ---------------------------------------------------------------------------------------
// `Decode` implementations
// ---------------------------------------------------------------------------------------

macro_rules! impl_decode_integer {
    ($($ty:ty),* $(,)?) => {
        $(
            impl Decode<Native> for $ty {
                fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
                    let column = next_column(row, offset)?;
                    <$ty>::try_from(parse_i64(&column)?).map_err(|_| {
                        column.error(concat!(
                            "a MySQL integer does not fit in ",
                            stringify!($ty)
                        ))
                    })
                }
            }
        )*
    };
}

impl_decode_integer!(i8, i16, i32, i64, u8, u16, u32);

impl Decode<Native> for u64 {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let column = next_column(row, offset)?;
        parse_u64(&column)
    }
}

impl Decode<Native> for u128 {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let column = next_column(row, offset)?;

        // A `DECIMAL(39,0)` column, which is what a `u128` column is declared as, arrives as
        // text. A negative or scaled value follows the same truncation rule as sqlx.
        if column.is_decimal()
            && let MySqlValue::Bytes(bytes) = column.value
        {
            let text = std::str::from_utf8(bytes).map_err(|err| column.error(err.to_string()))?;
            let digits = decimal_integer(text)
                // A negative value is an error, unless every digit is zero: a normalised
                // zero has no sign, which is what a decimal type reports for `-0`.
                .filter(|(negative, digits)| !negative || digits.bytes().all(|byte| byte == b'0'))
                .and_then(|(_, digits)| parse_u128(digits));
            return digits.ok_or_else(|| {
                column.error("the DECIMAL value is not a non-negative integer that fits in u128")
            });
        }

        // A value held in a real integer column still decodes through the integer path.
        Ok(u128::from(parse_u64(&column)?))
    }
}

impl Decode<Native> for i128 {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let column = next_column(row, offset)?;

        if column.is_decimal()
            && let MySqlValue::Bytes(bytes) = column.value
        {
            let text = std::str::from_utf8(bytes).map_err(|err| column.error(err.to_string()))?;
            return decimal_integer(text)
                .and_then(|(negative, digits)| parse_i128(digits, negative))
                .ok_or_else(|| {
                    column.error("the DECIMAL value is not an integer that fits in i128")
                });
        }

        parse_i64(&column).map(i128::from)
    }
}

impl Decode<Native> for bool {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        parse_bool(&next_column(row, offset)?)
    }
}

impl Decode<Native> for f32 {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        // MySQL sends `FLOAT` as either four or eight bytes; going through `f64` handles both.
        Ok(parse_f64(&next_column(row, offset)?)? as f32)
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
        Ok(column.bytes()?.to_vec())
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
        Ok(parse_naive_datetime(&next_column(row, offset)?)?.and_utc())
    }
}

impl Decode<Native> for serde_json::Value {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        parse_json(&next_column(row, offset)?)
    }
}

impl Decode<Native> for SqlInterval {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let column = next_column(row, offset)?;
        let text = parse_text(&column)?;
        text.trim()
            .parse()
            .map_err(|err: besu_core::ParseValueError| column.error(err.to_string()))
    }
}

impl Decode<Native> for Inet {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let column = next_column(row, offset)?;
        let text = parse_text(&column)?;
        Self::new(text.trim()).map_err(|err| column.error(err.to_string()))
    }
}

impl Decode<Native> for Cidr {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let column = next_column(row, offset)?;
        let text = parse_text(&column)?;
        Self::new(text.trim()).map_err(|err| column.error(err.to_string()))
    }
}

impl Decode<Native> for BitString {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        parse_bit_string(&next_column(row, offset)?)
    }
}

impl<T> Decode<Native> for Range<T>
where
    T: core::str::FromStr,
{
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let column = next_column(row, offset)?;
        let text = parse_text(&column)?;
        Range::parse_with(text.trim(), str::parse::<T>).map_err(|err| column.error(err.to_string()))
    }
}

impl Decode<Native> for Geometry {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let column = next_column(row, offset)?;
        // MySQL sends the internal format: a four-byte SRID and WKB.
        Self::from_mysql_bytes(column.bytes()?).map_err(|err| column.error(err.to_string()))
    }
}

impl Decode<Native> for Vector {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let column = next_column(row, offset)?;
        let text = parse_text(&column)?;
        text.trim()
            .parse()
            .map_err(|err: besu_core::ParseValueError| column.error(err.to_string()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn decimal_text_truncates_toward_zero() {
        assert_eq!(decimal_integer("1234"), Some((false, "1234")));
        assert_eq!(decimal_integer("-1234"), Some((true, "1234")));
        // A scale is dropped, not rounded.
        assert_eq!(decimal_integer("1.90"), Some((false, "1")));
        assert_eq!(decimal_integer("-98765432.99"), Some((true, "98765432")));
        assert_eq!(decimal_integer("1e10"), None);
        assert_eq!(decimal_integer(""), None);
    }

    #[test]
    fn a_wide_integer_is_parsed_without_an_intermediate_decimal() {
        assert_eq!(
            parse_u128("340282366920938463463374607431768211455"),
            Some(u128::MAX)
        );
        assert_eq!(
            parse_i128("170141183460469231731687303715884105728", true),
            Some(i128::MIN)
        );
        assert_eq!(
            parse_i128("340282366920938463463374607431768211456", false),
            None
        );
    }
}
