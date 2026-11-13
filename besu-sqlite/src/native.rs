//! A SQLite driver backed by [`rusqlite`](https://docs.rs/rusqlite), behind the `native`
//! feature.
//!
//! `rusqlite` is synchronous, so the driver owns a thread with the `rusqlite::Connection` on
//! it and talks to that thread over a request channel. A query sends its SQL text, its owned
//! parameters and a oneshot for the response; the async side awaits the oneshot. This is one
//! thread per [`Native`], not one blocking task per statement, which means a sequence of
//! statements costs a channel round trip rather than a thread-pool hand-off.
//!
//! A single connection is also what makes `:memory:` usable: there is exactly one database,
//! because there is exactly one connection. The cost is that there is no pool: every clone of
//! [`Native`] shares the same connection, and statements are serialised behind one another.
//! That is deliberate. Pooling, timeouts and health checks are the caller's problem until a
//! native pool exists; nothing here pretends to have one.
//!
//! Two channels do two different jobs. Requests travel over `std::sync::mpsc`, because the
//! worker blocks on `recv` and a futures channel has no blocking receive. Replies are
//! `futures_channel::oneshot`, because the async side awaits one and a oneshot receiver is
//! `Send` when its value is.

use std::{
    fmt,
    path::Path,
    sync::{Arc, mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};

use besu_core::{
    BitString, Cidr, Decode, Driver, Error, Geometry, Inet, Range, RowStream, Sql, SqlInterval,
    TransactionDriver, Value, Vector,
};
// `fix` and `from_utc_datetime` are trait methods on `chrono`'s offsets.
use chrono::{Offset as _, TimeZone as _};
use futures_channel::oneshot;
use rusqlite::params_from_iter;
use rusqlite::{
    Connection,
    types::{Value as SqlValue, ValueRef},
};

use crate::SQLite;

/// Everything the native SQLite driver can fail with.
///
/// A statement rejected by SQLite, a value that cannot be encoded or decoded, and a
/// connection thread that has stopped all arrive here, so a caller has one `?` type.
#[derive(Debug)]
#[non_exhaustive]
pub enum NativeError {
    /// SQLite rejected a statement, or the database file could not be opened.
    Sqlite(rusqlite::Error),
    /// A query carried a value this driver cannot encode, or a column could not be decoded.
    Core(Error),
    /// The connection thread has stopped, so no statement can reach the database.
    Closed,
    /// The connection thread could not be spawned.
    Thread(std::io::Error),
}

impl fmt::Display for NativeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(err) => write!(f, "{err}"),
            Self::Core(err) => write!(f, "{err}"),
            Self::Closed => f.write_str("the SQLite connection thread has stopped"),
            Self::Thread(err) => write!(f, "could not start the SQLite connection thread: {err}"),
        }
    }
}

impl core::error::Error for NativeError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Sqlite(err) => Some(err),
            Self::Core(err) => Some(err),
            Self::Closed => None,
            Self::Thread(err) => Some(err),
        }
    }
}

impl From<rusqlite::Error> for NativeError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Sqlite(err)
    }
}

impl From<Error> for NativeError {
    fn from(err: Error) -> Self {
        Self::Core(err)
    }
}

impl From<std::io::Error> for NativeError {
    fn from(err: std::io::Error) -> Self {
        Self::Thread(err)
    }
}

/// What a statement that returns no rows reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NativeOutput {
    rows_affected: u64,
    last_insert_rowid: i64,
}

impl NativeOutput {
    /// The number of rows the statement changed.
    pub const fn rows_affected(&self) -> u64 {
        self.rows_affected
    }

    /// The rowid of the most recent successful insert on this connection.
    pub const fn last_insert_rowid(&self) -> i64 {
        self.last_insert_rowid
    }
}

/// One row of a native SQLite result set.
///
/// The column names are shared by every row of one statement rather than copied per row, so
/// a wide result set does not pay for its header once per row.
#[derive(Clone, Debug)]
pub struct NativeRow {
    columns: Arc<[String]>,
    values: Vec<SqlValue>,
}

impl NativeRow {
    /// The number of values in the row.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether the row has no values.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Whether the value at `index` is `NULL`.
    ///
    /// An out-of-range index reports `true`, matching [`Driver::is_null`]: an outer-joined
    /// row that a driver hands back short must decode as `None`, not as an error.
    pub fn is_null(&self, index: usize) -> bool {
        matches!(self.values.get(index), None | Some(SqlValue::Null))
    }

    /// The names of the row's columns, in decode order.
    pub fn column_names(&self) -> Vec<String> {
        self.columns.to_vec()
    }
}

/// A SQLite driver backed by `rusqlite`, on a dedicated connection thread.
///
/// The constructors are synchronous because opening a file is: the connection is opened on
/// the calling thread, configured, and then moved to the thread that will own it. Cloning a
/// [`Native`] clones a handle to the same connection, so every clone shares one transaction
/// context.
#[derive(Clone)]
pub struct Native {
    inner: Arc<NativeInner>,
}

impl fmt::Debug for Native {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Native").finish_non_exhaustive()
    }
}

impl Native {
    /// Open a SQLite database file behind the native driver.
    ///
    /// The file is created if it does not exist, foreign keys are enabled, and the journal
    /// is switched to WAL. Those are the same defaults as [`crate::Sqlx::new`].
    pub fn new(path: impl AsRef<Path>) -> Result<Self, NativeError> {
        Self::open(path)
    }

    /// Open a SQLite database file, creating it if it does not exist.
    ///
    /// This is [`Native::new`] under the name that matches the other drivers.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, NativeError> {
        let connection = Connection::open(path)?;
        Self::configure(&connection, true)?;
        Self::spawn(connection)
    }

    /// Open an in-memory database.
    ///
    /// An in-memory database lives for exactly one connection, and this driver has exactly
    /// one, so every clone of the returned [`Native`] sees the same tables.
    pub fn in_memory() -> Result<Self, NativeError> {
        let connection = Connection::open_in_memory()?;
        Self::configure(&connection, false)?;
        Self::spawn(connection)
    }

    /// Take ownership of a `rusqlite` connection.
    ///
    /// The connection is moved to the driver's thread, so nothing else may use it afterwards.
    /// Foreign keys are enabled; the journal mode is left as the caller configured it.
    pub fn from_connection(connection: Connection) -> Result<Self, NativeError> {
        Self::configure(&connection, false)?;
        Self::spawn(connection)
    }

    /// Wrap this driver in a [`besu::Database`].
    pub fn database(self) -> besu::Database<Self> {
        besu::Database::new(self)
    }

    fn configure(connection: &Connection, wal: bool) -> Result<(), NativeError> {
        connection.execute_batch("PRAGMA foreign_keys = ON;")?;
        connection.busy_timeout(Duration::from_secs(5))?;
        if wal {
            // `journal_mode` always returns a row; `pragma_update` is the API that runs it
            // without asking the caller to consume one.
            connection.pragma_update(None, "journal_mode", "WAL")?;
        }
        Ok(())
    }

    fn spawn(connection: Connection) -> Result<Self, NativeError> {
        let (sender, receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("besu-sqlite".into())
            .spawn(move || worker(connection, receiver))?;
        Ok(Self {
            inner: Arc::new(NativeInner {
                sender: Some(sender),
                worker: Some(worker),
            }),
        })
    }

    fn send(&self, request: Request) -> Result<(), NativeError> {
        self.inner
            .sender
            .as_ref()
            .ok_or(NativeError::Closed)?
            .send(request)
            .map_err(|_| NativeError::Closed)
    }

    async fn call<R>(
        &self,
        request: impl FnOnce(oneshot::Sender<Result<R, NativeError>>) -> Request,
    ) -> Result<R, NativeError> {
        let (reply, response) = oneshot::channel();
        self.send(request(reply))?;
        response.await.unwrap_or(Err(NativeError::Closed))
    }
}

/// The handle to one thread's connection.
///
/// The worker owns the [`JoinHandle`] so that dropping the last [`Native`] can wait for the
/// thread to stop. `sender` is an `Option` for the same reason: the drop implementation has
/// to close the channel *before* joining, or the worker would never see the disconnect.
struct NativeInner {
    sender: Option<mpsc::Sender<Request>>,
    worker: Option<JoinHandle<()>>,
}

impl Drop for NativeInner {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            // The worker is only ever blocked on the channel, so this waits for at most the
            // statement it is currently running. Ignoring the result keeps a panicking
            // worker from propagating out of a destructor.
            let _ = worker.join();
        }
    }
}

/// A request to the connection thread.
///
/// Parameters are owned `rusqlite` values: they were encoded on the caller's side, so the
/// worker does no conversion and the statement is bound from values that already have the
/// storage class SQLite will use.
enum Request {
    Query {
        transaction: Option<u64>,
        sql: String,
        params: Vec<SqlValue>,
        reply: oneshot::Sender<Result<Vec<NativeRow>, NativeError>>,
    },
    Execute {
        transaction: Option<u64>,
        sql: String,
        params: Vec<SqlValue>,
        reply: oneshot::Sender<Result<NativeOutput, NativeError>>,
    },
    Begin {
        reply: oneshot::Sender<Result<u64, NativeError>>,
    },
    Commit {
        transaction: u64,
        reply: oneshot::Sender<Result<(), NativeError>>,
    },
    Rollback {
        transaction: u64,
        reply: oneshot::Sender<Result<(), NativeError>>,
    },
    Statements {
        statements: Vec<String>,
        reply: oneshot::Sender<Result<(), NativeError>>,
    },
}

/// The connection thread.
///
/// Every request is executed synchronously on the one connection, in the order it arrived.
/// A transaction is an id this loop hands out; a statement carrying that id is validated
/// against the open transaction before it runs, which is what stops a handle that was
/// already committed from writing into the next transaction.
fn worker(connection: Connection, receiver: mpsc::Receiver<Request>) {
    let mut open: Option<u64> = None;
    let mut next_transaction: u64 = 0;

    while let Ok(request) = receiver.recv() {
        match request {
            Request::Query {
                transaction,
                sql,
                params,
                reply,
            } => {
                let result = check_transaction(transaction, open)
                    .and_then(|()| query(&connection, &sql, &params));
                let _ = reply.send(result);
            }
            Request::Execute {
                transaction,
                sql,
                params,
                reply,
            } => {
                let result = check_transaction(transaction, open)
                    .and_then(|()| execute(&connection, &sql, &params));
                let _ = reply.send(result);
            }
            Request::Begin { reply } => {
                let result = if open.is_some() {
                    Err(NativeError::Core(Error::invalid_query(
                        "a transaction is already open on this SQLite connection",
                    )))
                } else {
                    match connection.execute_batch("BEGIN") {
                        Ok(()) => {
                            let id = next_transaction;
                            next_transaction = next_transaction.wrapping_add(1);
                            open = Some(id);
                            Ok(id)
                        }
                        Err(err) => Err(err.into()),
                    }
                };
                let _ = reply.send(result);
            }
            Request::Commit { transaction, reply } => {
                let result = if open != Some(transaction) {
                    Err(stale_transaction())
                } else {
                    match connection.execute_batch("COMMIT") {
                        Ok(()) => Ok(()),
                        Err(err) => {
                            // A failed `COMMIT` leaves the transaction open; rolling back
                            // keeps the connection usable instead of stranding it.
                            let _ = connection.execute_batch("ROLLBACK");
                            Err(err.into())
                        }
                    }
                };
                if open == Some(transaction) {
                    open = None;
                }
                let _ = reply.send(result);
            }
            Request::Rollback { transaction, reply } => {
                let result = if open != Some(transaction) {
                    Err(stale_transaction())
                } else {
                    connection
                        .execute_batch("ROLLBACK")
                        .map_err(NativeError::from)
                };
                if open == Some(transaction) {
                    open = None;
                }
                let _ = reply.send(result);
            }
            Request::Statements { statements, reply } => {
                let mut result = Ok(());
                for statement in &statements {
                    // `execute_batch` is the API that leaves a `PRAGMA` free to change the
                    // connection state the next statement sees, which is why the table
                    // rebuild can turn foreign keys off and back on in one request.
                    if let Err(err) = connection.execute_batch(statement) {
                        result = Err(err.into());
                        break;
                    }
                }
                let _ = reply.send(result);
            }
        }
    }
}

fn stale_transaction() -> NativeError {
    NativeError::Core(Error::invalid_query(
        "the transaction is no longer open on this connection",
    ))
}

fn check_transaction(transaction: Option<u64>, open: Option<u64>) -> Result<(), NativeError> {
    match transaction {
        None => Ok(()),
        Some(id) if open == Some(id) => Ok(()),
        Some(_) => Err(stale_transaction()),
    }
}

fn query(
    connection: &Connection,
    sql: &str,
    params: &[SqlValue],
) -> Result<Vec<NativeRow>, NativeError> {
    let mut statement = connection.prepare(sql)?;
    let columns: Arc<[String]> = statement
        .column_names()
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>()
        .into();
    let mut rows = statement.query(params_from_iter(params.iter()))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let mut values = Vec::with_capacity(columns.len());
        for index in 0..columns.len() {
            values.push(own(row.get_ref(index)?, index)?);
        }
        out.push(NativeRow {
            columns: Arc::clone(&columns),
            values,
        });
    }
    Ok(out)
}

fn execute(
    connection: &Connection,
    sql: &str,
    params: &[SqlValue],
) -> Result<NativeOutput, NativeError> {
    let mut statement = connection.prepare(sql)?;
    let affected = if statement.column_count() == 0 {
        statement.execute(params_from_iter(params.iter()))? as u64
    } else {
        // `execute` rejects a statement that returns rows. Draining them keeps a `RETURNING`
        // or a `PRAGMA` usable through `Driver::execute`, which is the path the migration
        // runner and the schema writer take.
        let mut rows = statement.query(params_from_iter(params.iter()))?;
        while rows.next()?.is_some() {}
        connection.changes()
    };
    Ok(NativeOutput {
        rows_affected: affected,
        last_insert_rowid: connection.last_insert_rowid(),
    })
}

/// Turn one borrowed column into an owned value.
///
/// This is the one copy a collected row makes: SQLite's text and blob memory belongs to the
/// statement, which is dropped when the request finishes, so a row that outlives it has to
/// own its bytes.
fn own(value: ValueRef<'_>, index: usize) -> Result<SqlValue, NativeError> {
    Ok(match value {
        ValueRef::Null => SqlValue::Null,
        ValueRef::Integer(value) => SqlValue::Integer(value),
        ValueRef::Real(value) => SqlValue::Real(value),
        ValueRef::Text(bytes) => {
            SqlValue::Text(String::from_utf8(bytes.to_vec()).map_err(|_| {
                NativeError::Core(Error::decode(index, "the SQLite text is not valid UTF-8"))
            })?)
        }
        ValueRef::Blob(bytes) => SqlValue::Blob(bytes.to_vec()),
    })
}

impl Driver for Native {
    type Dialect = SQLite;
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
        let mut offset = index;
        <String as Decode<Self>>::decode(row, &mut offset)
    }

    async fn query(&self, sql: &Sql<SQLite>) -> Result<Vec<Self::Row>, Self::Error> {
        let params = arguments(sql)?;
        let sql = sql.sql().to_owned();
        self.call(|reply| Request::Query {
            transaction: None,
            sql,
            params,
            reply,
        })
        .await
    }

    async fn execute(&self, sql: &Sql<SQLite>) -> Result<Self::Output, Self::Error> {
        let params = arguments(sql)?;
        let sql = sql.sql().to_owned();
        self.call(|reply| Request::Execute {
            transaction: None,
            sql,
            params,
            reply,
        })
        .await
    }

    /// Collect the rows on the connection thread and hand them back as a stream.
    ///
    /// A real cursor cannot leave the worker: the statement borrows the connection, and
    /// holding it across an await would stop every other statement on the same connection
    /// from running. Collecting is the honest trade for one connection; the rows are owned,
    /// so the stream satisfies [`RowStream`]'s lifetime without borrowing the statement.
    async fn query_stream<'a>(
        &'a self,
        sql: &'a Sql<SQLite>,
    ) -> Result<RowStream<'a, Self>, Self::Error> {
        let rows = self.query(sql).await?;
        let items = rows.into_iter().map(Ok).collect::<Vec<_>>();
        Ok(Box::pin(besu_core::futures_stream::iter(items)) as RowStream<'a, Self>)
    }
}

impl besu::SchemaDriver for Native {
    type Ddl = crate::SQLiteDdl;

    async fn introspect(&self) -> Result<besu::schema::Schema, Self::Error> {
        let db = besu::Database::new(self.clone());
        crate::introspect::introspect(&db).await
    }

    /// Run every schema statement on the one connection, in order.
    ///
    /// SQLite's table rebuild toggles `PRAGMA foreign_keys`, which is per-connection state.
    /// The default implementation would send each statement as its own request, and although
    /// this driver has a single connection it is clearer to send the whole list as one
    /// request: nothing else can interleave with a rebuild.
    async fn execute_statements(&self, statements: &[String]) -> Result<(), Self::Error> {
        let statements = statements.to_vec();
        self.call(|reply| Request::Statements { statements, reply })
            .await
    }
}

/// A transaction on the native driver's one connection.
///
/// Dropping the handle without committing rolls back, which is what the sqlx driver does
/// through sqlx's own transaction guard. The lifetime ties the handle to the database it was
/// begun from, so it cannot outlive the `&Database`.
pub struct NativeTransaction<'a> {
    native: Native,
    transaction: u64,
    finished: bool,
    marker: core::marker::PhantomData<&'a Native>,
}

impl fmt::Debug for NativeTransaction<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeTransaction")
            .field("transaction", &self.transaction)
            .finish_non_exhaustive()
    }
}

impl Drop for NativeTransaction<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.finished = true;
            let (reply, _response) = oneshot::channel();
            let _ = self.native.send(Request::Rollback {
                transaction: self.transaction,
                reply,
            });
        }
    }
}

impl TransactionDriver for Native {
    type Transaction<'a> = NativeTransaction<'a>;

    async fn begin<'a>(&'a self) -> Result<Self::Transaction<'a>, Self::Error> {
        let transaction = self.call(|reply| Request::Begin { reply }).await?;
        Ok(NativeTransaction {
            native: self.clone(),
            transaction,
            finished: false,
            marker: core::marker::PhantomData,
        })
    }

    async fn commit<'a>(mut tx: Self::Transaction<'a>) -> Result<(), Self::Error> {
        tx.finished = true;
        let transaction = tx.transaction;
        tx.native
            .call(|reply| Request::Commit { transaction, reply })
            .await
    }

    async fn rollback<'a>(mut tx: Self::Transaction<'a>) -> Result<(), Self::Error> {
        tx.finished = true;
        let transaction = tx.transaction;
        tx.native
            .call(|reply| Request::Rollback { transaction, reply })
            .await
    }

    async fn txn_query<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &Sql<SQLite>,
    ) -> Result<Vec<Self::Row>, Self::Error> {
        let params = arguments(sql)?;
        let sql = sql.sql().to_owned();
        let transaction = tx.transaction;
        tx.native
            .call(|reply| Request::Query {
                transaction: Some(transaction),
                sql,
                params,
                reply,
            })
            .await
    }

    async fn txn_execute<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &Sql<SQLite>,
    ) -> Result<Self::Output, Self::Error> {
        let params = arguments(sql)?;
        let sql = sql.sql().to_owned();
        let transaction = tx.transaction;
        tx.native
            .call(|reply| Request::Execute {
                transaction: Some(transaction),
                sql,
                params,
                reply,
            })
            .await
    }

    async fn txn_query_stream<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &'c Sql<SQLite>,
    ) -> Result<RowStream<'c, Self>, Self::Error> {
        let rows = Self::txn_query(tx, sql).await?;
        let items = rows.into_iter().map(Ok).collect::<Vec<_>>();
        Ok(Box::pin(besu_core::futures_stream::iter(items)) as RowStream<'c, Self>)
    }
}

// ---------------------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------------------

fn format_text(capacity: usize, write: impl FnOnce(&mut String) -> fmt::Result) -> String {
    let mut out = String::with_capacity(capacity);
    // Writing into a `String` cannot fail; the crate forbids panic paths, so the result is
    // discarded rather than `expect`-ed.
    let _ = write(&mut out);
    out
}

/// Encode a statement's parameters into owned SQLite values.
///
/// Each [`Value`] is encoded exactly once. The mapping mirrors the sqlx driver column for
/// column: SQLite has no integer wider than `i64`, no decimal, interval, address, bit-string,
/// range, spatial or vector type, so every one of those is its exact text (or, for a spatial
/// value, its EWKB bytes).
fn arguments(sql: &Sql<SQLite>) -> Result<Vec<SqlValue>, NativeError> {
    let mut params = Vec::with_capacity(sql.args().len());
    for value in sql.args() {
        params.push(match value {
            Value::Null => SqlValue::Null,
            Value::Bool(v) => SqlValue::Integer(i64::from(*v)),
            Value::Int(v) => SqlValue::Integer(*v),
            // The dialect already rejects a `u64` above `i64::MAX`; this keeps the same
            // saturation the sqlx driver applies if a hand-built `Sql` gets past it.
            Value::UInt(v) => SqlValue::Integer(i64::try_from(*v).unwrap_or(i64::MAX)),
            // A 128-bit column is declared `TEXT`, because SQLite has no integer that wide.
            Value::UInt128(v) => SqlValue::Text(v.to_string()),
            Value::Int128(v) => SqlValue::Text(v.to_string()),
            Value::Float(v) => SqlValue::Real(*v),
            Value::Text(v) | Value::EnumLabel(v) | Value::Decimal(v) => SqlValue::Text(v.clone()),
            Value::Bytes(v) => SqlValue::Blob(v.clone()),
            Value::Uuid(v) => SqlValue::Blob(v.as_bytes().to_vec()),
            Value::Date(v) => SqlValue::Text(format_text(16, |out| v.format("%F").write_to(out))),
            Value::Time(v) => {
                SqlValue::Text(format_text(16, |out| v.format("%T%.f").write_to(out)))
            }
            Value::Timestamp(v) => {
                SqlValue::Text(format_text(32, |out| v.format("%F %T%.f").write_to(out)))
            }
            // `rusqlite` has no timestamp encoder to defer to; this is the same RFC 3339
            // spelling sqlx's chrono encoder writes.
            Value::TimestampTz(v) => {
                SqlValue::Text(v.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, false))
            }
            Value::Json(v) => SqlValue::Text(std::str::from_utf8(v).unwrap_or("null").to_owned()),
            Value::Interval(v) => SqlValue::Text(v.to_string()),
            Value::Inet(v) => SqlValue::Text(v.as_str().to_owned()),
            Value::Cidr(v) => SqlValue::Text(v.as_str().to_owned()),
            Value::Bit(v) => SqlValue::Text(v.as_str().to_owned()),
            Value::Range { range, .. } => SqlValue::Text(Value::range_text(range)),
            Value::Geometry(v) => SqlValue::Blob(v.ewkb().to_vec()),
            Value::Vector(v) => SqlValue::Text(v.to_string()),
            Value::Array { .. } => {
                return Err(NativeError::Core(Error::encode(
                    value.display(),
                    "`sqlite` has no array type; store the elements as JSON instead",
                )));
            }
            // `Value` is `#[non_exhaustive]`: a future variant fails loudly rather than
            // silently becoming `NULL`.
            _ => {
                return Err(NativeError::Core(Error::encode(
                    value.display(),
                    "this driver does not know how to bind this value",
                )));
            }
        });
    }
    Ok(params)
}

// ---------------------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------------------

/// Take the next column, advancing the offset, or report the column that was missing.
fn column<'a>(
    row: &'a NativeRow,
    offset: &mut usize,
) -> Result<(usize, &'a SqlValue), NativeError> {
    let index = *offset;
    *offset += 1;
    match row.values.get(index) {
        Some(value) => Ok((index, value)),
        None => Err(NativeError::Core(Error::decode(
            index,
            "the row has no column at this position",
        ))),
    }
}

/// The `i64` SQLite reports for a value.
///
/// SQLite coerces: a real is truncated, text is read as a decimal prefix (with saturation),
/// and `NULL` is zero. That is exactly what `sqlite3_value_int64` does, which is what the
/// sqlx driver reaches through, so a value stored by one driver decodes the same in the
/// other.
fn as_i64(value: &SqlValue) -> i64 {
    match value {
        SqlValue::Null => 0,
        SqlValue::Integer(value) => *value,
        SqlValue::Real(value) => *value as i64,
        SqlValue::Text(text) => int_prefix(text.as_bytes()),
        SqlValue::Blob(bytes) => int_prefix(bytes),
    }
}

fn int_prefix(bytes: &[u8]) -> i64 {
    let mut index = 0;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    let negative = match bytes.get(index) {
        Some(b'-') => {
            index += 1;
            true
        }
        Some(b'+') => {
            index += 1;
            false
        }
        _ => false,
    };

    let mut value: i128 = 0;
    let mut digits = 0;
    while let Some(digit) = bytes.get(index).filter(|byte| byte.is_ascii_digit()) {
        // Stop growing once the value is already too large for an `i64`; the final clamp
        // saturates it the way SQLite does. This keeps a million-digit input from
        // overflowing the accumulator.
        if value <= i64::MAX as i128 {
            value = value * 10 + i128::from(digit - b'0');
        }
        digits += 1;
        index += 1;
    }

    if digits == 0 {
        return 0;
    }
    let value = if negative { -value } else { value };
    value.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

/// The `f64` SQLite reports for a value.
fn as_f64(value: &SqlValue) -> f64 {
    match value {
        SqlValue::Null => 0.0,
        SqlValue::Integer(value) => *value as f64,
        SqlValue::Real(value) => *value,
        SqlValue::Text(text) => float_prefix(text),
        SqlValue::Blob(bytes) => std::str::from_utf8(bytes)
            .ok()
            .map(float_prefix)
            .unwrap_or(0.0),
    }
}

fn float_prefix(text: &str) -> f64 {
    let bytes = text.as_bytes();
    let mut start = 0;
    while bytes.get(start).is_some_and(u8::is_ascii_whitespace) {
        start += 1;
    }
    let mut index = start;
    if matches!(bytes.get(index), Some(b'+') | Some(b'-')) {
        index += 1;
    }
    let mut digits = 0;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
        digits += 1;
    }
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return 0.0;
    }
    let mantissa_end = index;
    if matches!(bytes.get(index), Some(b'e') | Some(b'E')) {
        let mut exponent = index + 1;
        if matches!(bytes.get(exponent), Some(b'+') | Some(b'-')) {
            exponent += 1;
        }
        let exponent_start = exponent;
        while bytes.get(exponent).is_some_and(u8::is_ascii_digit) {
            exponent += 1;
        }
        index = if exponent > exponent_start {
            exponent
        } else {
            mantissa_end
        };
    }
    text[start..index].parse().unwrap_or(0.0)
}

/// The text SQLite reports for a value, which is what `Decode<String>` reads.
fn as_text(value: &SqlValue, index: usize) -> Result<String, NativeError> {
    match value {
        SqlValue::Null => Ok(String::new()),
        SqlValue::Integer(value) => Ok(value.to_string()),
        SqlValue::Real(value) => Ok(real_text(*value)),
        SqlValue::Text(text) => Ok(text.clone()),
        SqlValue::Blob(bytes) => String::from_utf8(bytes.clone()).map_err(|_| {
            NativeError::Core(Error::decode(index, "the SQLite blob is not valid UTF-8"))
        }),
    }
}

/// Format a real the way SQLite's text conversion does for the common cases.
///
/// SQLite writes a whole-number real as `1.0`, not `1`; beyond that it uses `%!.15g`, whose
/// exponent spelling can differ from Rust's shortest round-trip. The difference is only
/// observable when a real is decoded into a `String`, which is rare; the round-trip through
/// `f64` is exact either way.
fn real_text(value: f64) -> String {
    if value.is_finite() && value == value.trunc() && value.abs() < 1e15 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}

/// The bytes SQLite reports for a value, which is what `Decode<Vec<u8>>` reads.
fn as_bytes(value: &SqlValue) -> Vec<u8> {
    match value {
        SqlValue::Null => Vec::new(),
        SqlValue::Integer(value) => value.to_string().into_bytes(),
        SqlValue::Real(value) => real_text(*value).into_bytes(),
        SqlValue::Text(text) => text.as_bytes().to_vec(),
        SqlValue::Blob(bytes) => bytes.clone(),
    }
}

macro_rules! impl_decode_int {
    ($($ty:ty),* $(,)?) => {
        $(
            impl Decode<Native> for $ty {
                fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
                    let (index, value) = column(row, offset)?;
                    <$ty>::try_from(as_i64(value)).map_err(|_| {
                        NativeError::Core(Error::decode(
                            index,
                            concat!("a SQLite integer is out of range for ", stringify!($ty)),
                        ))
                    })
                }
            }
        )*
    };
}

impl_decode_int!(i8, i16, i32, u8, u16, u32);

impl Decode<Native> for i64 {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let (_, value) = column(row, offset)?;
        Ok(as_i64(value))
    }
}

impl Decode<Native> for bool {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let (_, value) = column(row, offset)?;
        Ok(as_i64(value) != 0)
    }
}

impl Decode<Native> for f64 {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let (_, value) = column(row, offset)?;
        Ok(as_f64(value))
    }
}

impl Decode<Native> for f32 {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let (_, value) = column(row, offset)?;
        Ok(as_f64(value) as f32)
    }
}

impl Decode<Native> for String {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let (index, value) = column(row, offset)?;
        as_text(value, index)
    }
}

impl Decode<Native> for Vec<u8> {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let (_, value) = column(row, offset)?;
        Ok(as_bytes(value))
    }
}

impl Decode<Native> for uuid::Uuid {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let (index, value) = column(row, offset)?;
        Self::from_slice(&as_bytes(value))
            .map_err(|err| NativeError::Core(Error::decode(index, err.to_string())))
    }
}

impl Decode<Native> for u64 {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let (index, value) = column(row, offset)?;
        u64::try_from(as_i64(value)).map_err(|_| {
            NativeError::Core(Error::decode(
                index,
                "a negative SQLite integer cannot be decoded as u64",
            ))
        })
    }
}

/// Decode a 128-bit integer stored in a `TEXT` column as its decimal digits.
///
/// An integer storage class is accepted too, so a hand-written `SELECT` that happens to
/// produce one still decodes. This mirrors the sqlx driver's `decode_128`.
fn decode_128<T>(row: &NativeRow, offset: &mut usize, what: &str) -> Result<T, NativeError>
where
    T: TryFrom<i64> + core::str::FromStr,
{
    let index = *offset;
    *offset += 1;
    let Some(value) = row.values.get(index) else {
        return Err(NativeError::Core(Error::decode(
            index,
            "the row has no column at this position",
        )));
    };

    if let Ok(text) = as_text(value, index) {
        return text.trim().parse().map_err(|_| {
            NativeError::Core(Error::decode(
                index,
                format!("the value is not a valid {what}"),
            ))
        });
    }

    T::try_from(as_i64(value)).map_err(|_| {
        NativeError::Core(Error::decode(
            index,
            format!("a SQLite integer is out of range for {what}"),
        ))
    })
}

impl Decode<Native> for u128 {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        decode_128(row, offset, "u128")
    }
}

impl Decode<Native> for i128 {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        decode_128(row, offset, "i128")
    }
}

impl Decode<Native> for serde_json::Value {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let (index, value) = column(row, offset)?;
        let text = as_text(value, index)?;
        serde_json::from_str(&text)
            .map_err(|err| NativeError::Core(Error::decode(index, err.to_string())))
    }
}

impl Decode<Native> for SqlInterval {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let (index, value) = column(row, offset)?;
        let text = as_text(value, index)?;
        text.trim()
            .parse()
            .map_err(|err: besu_core::ParseValueError| {
                NativeError::Core(Error::decode(index, err.to_string()))
            })
    }
}

impl Decode<Native> for Inet {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let (index, value) = column(row, offset)?;
        let text = as_text(value, index)?;
        Self::new(text.trim())
            .map_err(|err| NativeError::Core(Error::decode(index, err.to_string())))
    }
}

impl Decode<Native> for Cidr {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let (index, value) = column(row, offset)?;
        let text = as_text(value, index)?;
        Self::new(text.trim())
            .map_err(|err| NativeError::Core(Error::decode(index, err.to_string())))
    }
}

impl Decode<Native> for BitString {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let (index, value) = column(row, offset)?;
        let text = as_text(value, index)?;
        Self::new(text.trim())
            .map_err(|err| NativeError::Core(Error::decode(index, err.to_string())))
    }
}

impl<T> Decode<Native> for Range<T>
where
    T: core::str::FromStr,
{
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let (index, value) = column(row, offset)?;
        let text = as_text(value, index)?;
        Range::parse_with(text.trim(), str::parse::<T>)
            .map_err(|err| NativeError::Core(Error::decode(index, err.to_string())))
    }
}

impl Decode<Native> for Geometry {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let (index, value) = column(row, offset)?;
        Self::from_ewkb(as_bytes(value))
            .map_err(|err| NativeError::Core(Error::decode(index, err.to_string())))
    }
}

impl Decode<Native> for Vector {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let (index, value) = column(row, offset)?;
        let text = as_text(value, index)?;
        text.trim()
            .parse()
            .map_err(|err: besu_core::ParseValueError| {
                NativeError::Core(Error::decode(index, err.to_string()))
            })
    }
}

impl Decode<Native> for chrono::NaiveDate {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let (index, value) = column(row, offset)?;
        let text = as_text(value, index)?;
        Self::parse_from_str(&text, "%F")
            .map_err(|err| NativeError::Core(Error::decode(index, err.to_string())))
    }
}

impl Decode<Native> for chrono::NaiveTime {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let (index, value) = column(row, offset)?;
        let text = as_text(value, index)?;

        // The same spellings sqlx accepts, most likely first.
        const FORMATS: &[&str] = &["%T.f", "%T%.f", "%R", "%RZ", "%T%.fZ", "%R%:z", "%T%.f%:z"];
        for format in FORMATS {
            if let Ok(time) = Self::parse_from_str(&text, format) {
                return Ok(time);
            }
        }

        Err(NativeError::Core(Error::decode(
            index,
            format!("invalid time: {text}"),
        )))
    }
}

impl Decode<Native> for chrono::NaiveDateTime {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let (index, value) = column(row, offset)?;
        decode_datetime(value, index).map(|datetime| datetime.naive_local())
    }
}

impl Decode<Native> for chrono::DateTime<chrono::Utc> {
    fn decode(row: &NativeRow, offset: &mut usize) -> Result<Self, NativeError> {
        let (index, value) = column(row, offset)?;
        decode_datetime(value, index)
            .map(|datetime| chrono::Utc.from_utc_datetime(&datetime.naive_utc()))
    }
}

/// Read a timestamp from whichever storage class the column produced.
///
/// sqlx accepts text in a list of date/time spellings, an integer Unix timestamp and a real
/// Julian day; this accepts the same three so a file written by the sqlx driver reads back
/// identically.
fn decode_datetime(
    value: &SqlValue,
    index: usize,
) -> Result<chrono::DateTime<chrono::FixedOffset>, NativeError> {
    let parsed = match value {
        SqlValue::Text(_) => {
            let text = as_text(value, index)?;
            parse_datetime(&text)
        }
        SqlValue::Integer(timestamp) => chrono::DateTime::from_timestamp(*timestamp, 0)
            .map(|datetime| chrono::Utc.fix().from_utc_datetime(&datetime.naive_utc())),
        SqlValue::Real(julian) => decode_julian(*julian),
        SqlValue::Null => None,
        SqlValue::Blob(_) => {
            let text = as_text(value, index)?;
            parse_datetime(&text)
        }
    };

    parsed.ok_or_else(|| {
        NativeError::Core(Error::decode(
            index,
            format!(
                "invalid datetime: {}",
                as_text(value, index).unwrap_or_default()
            ),
        ))
    })
}

fn parse_datetime(text: &str) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    if let Ok(datetime) = chrono::DateTime::parse_from_rfc3339(text) {
        return Some(datetime);
    }

    const FORMATS: &[&str] = &[
        "%F %T%.f",
        "%F %R",
        "%F %RZ",
        "%F %R%:z",
        "%F %T%.fZ",
        "%F %T%.f%:z",
        "%FT%R",
        "%FT%RZ",
        "%FT%R%:z",
        "%FT%T%.f",
        "%FT%T%.fZ",
        "%FT%T%.f%:z",
    ];
    for format in FORMATS {
        if let Ok(datetime) = chrono::DateTime::parse_from_str(text, format) {
            return Some(datetime);
        }
        if let Ok(datetime) = chrono::NaiveDateTime::parse_from_str(text, format) {
            return Some(chrono::Utc.fix().from_utc_datetime(&datetime));
        }
    }

    None
}

fn decode_julian(value: f64) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    let epoch_in_julian_days = 2_440_587.5;
    let seconds_in_day = 86_400.0;
    let timestamp = (value - epoch_in_julian_days) * seconds_in_day;
    if !timestamp.is_finite() {
        return None;
    }

    let seconds = timestamp.trunc() as i64;
    let nanos = (timestamp.fract() * 1e9).abs() as u32;
    chrono::DateTime::from_timestamp(seconds, nanos)
        .map(|datetime| chrono::Utc.fix().from_utc_datetime(&datetime.naive_utc()))
}
