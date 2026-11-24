use std::{path::Path, str::from_utf8};

use besu_core::{
    BitString, Cidr, Decode, Driver, Error, Geometry, Inet, Range, Sql, SqlInterval,
    TransactionDriver, Value, Vector,
};
use besu_schema::Ddl;
use sqlx::{
    Arguments, Column as _, Row, Sqlite, SqlitePool,
    pool::PoolConnection,
    sqlite::{
        SqliteArguments, SqliteConnectOptions, SqlitePoolOptions, SqliteQueryResult, SqliteRow,
    },
};

use crate::SQLite;
use crate::SQLiteDdl;
use crate::ddl::{
    REBUILD_BEGIN, REBUILD_COMMIT, REBUILD_FOREIGN_KEYS_OFF, REBUILD_FOREIGN_KEYS_ON,
    is_foreign_key_check, split_rebuild,
};

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

    /// Open a database from a connection string such as `sqlite://app.db`.
    ///
    /// The same sensible defaults as [`Sqlx::new`] are applied on top of whatever the URL
    /// specifies, so a missing file is created and foreign keys are enforced.
    pub async fn connect(url: &str) -> Result<Self, sqlx::Error> {
        let options = url
            .parse::<SqliteConnectOptions>()?
            .create_if_missing(true)
            .foreign_keys(true);
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

fn add<'q, T>(arguments: &mut SqliteArguments<'q>, value: T) -> Result<(), sqlx::Error>
where
    T: 'q + sqlx::Encode<'q, Sqlite> + sqlx::Type<Sqlite>,
{
    arguments.add(value).map_err(sqlx::Error::Encode)
}

/// Format a value into a `String` that already has room for the whole result.
///
/// chrono's `Display` for a `DelayedFormat` renders into a fresh `String` and then copies
/// that into the formatter, which costs a date three allocations. `write_to` writes into a
/// buffer of the right size instead, so it costs one. The format strings used below are the
/// ones sqlx itself uses, so the bytes bound are identical.
fn text(capacity: usize, write: impl FnOnce(&mut String) -> core::fmt::Result) -> String {
    let mut out = String::with_capacity(capacity);
    // Writing into a `String` cannot fail; ignoring the `fmt::Result` rather than
    // `expect`-ing keeps this crate free of panic paths, which is a workspace-wide rule.
    let _ = write(&mut out);
    out
}

fn arguments<'q>(sql: &'q Sql<SQLite>) -> Result<SqliteArguments<'q>, sqlx::Error> {
    let mut arguments = SqliteArguments::default();
    // The argument count is already known, so the vector behind `SqliteArguments` can be
    // sized once instead of doubling three or four times on the way to a wide statement.
    arguments.reserve(sql.args().len(), 0);
    for value in sql.args() {
        match value {
            Value::Null => add(&mut arguments, None::<i64>)?,
            Value::Bool(v) => add(&mut arguments, *v)?,
            Value::Int(v) => add(&mut arguments, *v)?,
            Value::UInt(v) => add(&mut arguments, i64::try_from(*v).unwrap_or(i64::MAX))?,
            // A 128-bit column is declared `TEXT`, so the exact decimal digits are bound as
            // text rather than squeezed through SQLite's signed 64-bit integer. The decimal
            // text is the one allocation it costs, and there is no cheaper option: SQLite has
            // no 128-bit type and the argument must own its bytes.
            Value::UInt128(v) => add(&mut arguments, v.to_string())?,
            Value::Int128(v) => add(&mut arguments, v.to_string())?,
            Value::Float(v) => add(&mut arguments, *v)?,
            Value::Text(v) => add(&mut arguments, v.as_str())?,
            // SQLite stores an enum's label in a `TEXT` column, so it binds like text.
            Value::EnumLabel(v) => add(&mut arguments, v.as_str())?,
            // SQLite has no decimal type: `DECIMAL` is declared as `TEXT`, so the exact
            // digits are bound as text and never pass through a float.
            Value::Decimal(v) => add(&mut arguments, v.as_str())?,
            Value::Bytes(v) => add(&mut arguments, v.as_slice())?,
            // sqlx's `Uuid` encoder copies the sixteen bytes into an owned `Vec`. Binding the
            // same bytes as a borrowed blob writes an identical SQLite `BLOB` with no copy.
            Value::Uuid(v) => add(&mut arguments, v.as_bytes().as_slice())?,
            Value::Date(v) => add(&mut arguments, text(16, |out| v.format("%F").write_to(out)))?,
            Value::Time(v) => add(
                &mut arguments,
                text(16, |out| v.format("%T%.f").write_to(out)),
            )?,
            Value::Timestamp(v) => add(
                &mut arguments,
                text(32, |out| v.format("%F %T%.f").write_to(out)),
            )?,
            // sqlx's own encoder for a zoned timestamp already sizes its `String` up front, so
            // there is nothing to win here; the naive ones are the ones that grow from empty.
            Value::TimestampTz(v) => add(&mut arguments, *v)?,
            // SQLite has no interval, address or bit-string type: the column is `TEXT` and
            // the canonical text is the exact value, so it round-trips unchanged.
            Value::Interval(v) => add(&mut arguments, v.to_string())?,
            Value::Inet(v) => add(&mut arguments, v.as_str())?,
            Value::Cidr(v) => add(&mut arguments, v.as_str())?,
            Value::Bit(v) => add(&mut arguments, v.as_str())?,
            // SQLite has no range type: the column is `TEXT` and the canonical spelling is
            // what is stored.
            Value::Range { range, .. } => add(&mut arguments, Value::range_text(range))?,
            // SQLite has no spatial type: the EWKB bytes go into the `BLOB` column.
            Value::Geometry(v) => add(&mut arguments, v.ewkb())?,
            // SQLite has no vector type; the `[1,2,3]` text is the storage.
            Value::Vector(v) => add(&mut arguments, v.to_string())?,
            // `add` takes a `&str`, and sqlx stores it as `Cow::Borrowed`, so the JSON bytes
            // are not copied. Binding the `&[u8]` instead would cost the same and lose the
            // column's `TEXT` affinity, which JSON functions rely on. Bytes that are not
            // UTF-8 are not a JSON document, so they fail rather than becoming `NULL`.
            Value::Json(v) => add(
                &mut arguments,
                from_utf8(v).map_err(|_| {
                    <Sqlx as Driver>::error(Error::encode(
                        value.display(),
                        "the JSON value is not valid UTF-8",
                    ))
                })?,
            )?,
            // SQLite has no array type. Binding `NULL` instead would turn a programming
            // mistake into a missing value, so the statement fails before it is sent.
            Value::Array { .. } => {
                return Err(<Sqlx as Driver>::error(Error::encode(
                    value.display(),
                    "`sqlite` has no array type; store the elements as JSON instead",
                )));
            }
            // `Value` is `#[non_exhaustive]`: a future variant must fail loudly rather than
            // silently bind `NULL`.
            _ => {
                return Err(<Sqlx as Driver>::error(Error::encode(
                    value.display(),
                    "this driver does not know how to bind this value",
                )));
            }
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
pub fn arguments_for_bench(sql: &Sql<SQLite>) -> Result<SqliteArguments<'_>, sqlx::Error> {
    arguments(sql)
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

    async fn query_stream<'a>(
        &'a self,
        sql: &'a Sql<SQLite>,
    ) -> Result<besu_core::RowStream<'a, Self>, Self::Error> {
        let arguments = arguments(sql)?;
        Ok(sqlx::query_with(sql.sql(), arguments).fetch(&self.pool))
    }
}

impl besu::SchemaDriver for Sqlx {
    type Ddl = crate::SQLiteDdl;

    async fn introspect(&self) -> Result<besu::schema::Schema, Self::Error> {
        let db = besu::Database::new(self.clone());
        crate::introspect::introspect(&db).await
    }

    /// Run every schema statement on one connection.
    ///
    /// SQLite's table rebuild toggles `PRAGMA foreign_keys`, which applies to the connection
    /// that ran it. Through a pool, the `DROP TABLE` would otherwise land on a connection
    /// that still has foreign keys enabled and cascade into child rows.
    async fn execute_statements(&self, mut statements: &[String]) -> Result<(), Self::Error> {
        let mut connection = self.pool.acquire().await?;
        while let Some((statement, rest)) = statements.split_first() {
            if let Some((body, rest)) = split_rebuild(statements) {
                run_rebuild(&mut connection, body).await?;
                statements = rest;
            } else {
                sqlx::query(statement).execute(&mut *connection).await?;
                statements = rest;
            }
        }
        Ok(())
    }
}

/// Quarantine a pooled connection when cancellation interrupts the rebuild or its cleanup.
/// Closing the connection rolls back any open transaction; a successful cleanup reuses it.
struct RebuildConnection<'a> {
    connection: &'a mut PoolConnection<Sqlite>,
    close_on_drop: bool,
}

impl Drop for RebuildConnection<'_> {
    fn drop(&mut self) {
        if self.close_on_drop {
            self.connection.close_on_drop();
        }
    }
}

impl RebuildConnection<'_> {
    async fn execute(&mut self, statement: &str) -> Result<(), sqlx::Error> {
        sqlx::query(statement)
            .execute(&mut **self.connection)
            .await?;
        Ok(())
    }
}

async fn run_rebuild(
    connection: &mut PoolConnection<Sqlite>,
    statements: &[String],
) -> Result<(), sqlx::Error> {
    let foreign_keys: bool = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&mut **connection)
        .await?;
    let legacy_alter_table: bool = sqlx::query_scalar("PRAGMA legacy_alter_table")
        .fetch_one(&mut **connection)
        .await?;
    let mut guard = RebuildConnection {
        connection,
        close_on_drop: true,
    };
    guard.execute(REBUILD_FOREIGN_KEYS_OFF).await?;
    guard.execute("PRAGMA legacy_alter_table = ON").await?;
    let result = match guard.execute(REBUILD_BEGIN).await {
        Ok(()) => {
            let result = async {
                let views = valid_views(guard.connection).await?;
                for statement in statements {
                    execute_rebuild_statement(&mut guard, statement).await?;
                }
                validate_views(&mut guard, &views).await?;
                guard.execute(REBUILD_COMMIT).await
            }
            .await;
            // Only roll back a transaction we successfully opened. A failed BEGIN could
            // mean a caller already opened one, which is not ours to roll back.
            if result.is_err() && guard.execute("ROLLBACK").await.is_err() {
                return result;
            }
            result
        }
        Err(err) => Err(err),
    };
    let restore_legacy = guard
        .execute(if legacy_alter_table {
            "PRAGMA legacy_alter_table = ON"
        } else {
            "PRAGMA legacy_alter_table = OFF"
        })
        .await;
    let restore_foreign_keys = guard
        .execute(if foreign_keys {
            REBUILD_FOREIGN_KEYS_ON
        } else {
            REBUILD_FOREIGN_KEYS_OFF
        })
        .await;
    guard.close_on_drop = restore_legacy.is_err() || restore_foreign_keys.is_err();
    result.and(restore_legacy).and(restore_foreign_keys)
}

/// Views that can be prepared before a rebuild. Invalid pre-existing views are ignored;
/// only a view that used to work and is broken by the rebuild should fail the push.
async fn valid_views(connection: &mut PoolConnection<Sqlite>) -> Result<Vec<String>, sqlx::Error> {
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'view' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .fetch_all(&mut **connection)
    .await?;
    let mut views = Vec::new();
    for name in names {
        let sql = format!("SELECT * FROM {} LIMIT 0", SQLiteDdl.quote(&name));
        if sqlx::query(&sql).fetch_all(&mut **connection).await.is_ok() {
            views.push(name);
        }
    }
    Ok(views)
}

async fn validate_views(
    guard: &mut RebuildConnection<'_>,
    views: &[String],
) -> Result<(), sqlx::Error> {
    for view in views {
        let sql = format!("SELECT * FROM {} LIMIT 0", SQLiteDdl.quote(view));
        sqlx::query(&sql)
            .fetch_all(&mut **guard.connection)
            .await
            .map_err(|error| {
                sqlx::Error::Protocol(format!(
                    "view {view:?} is incompatible with the rebuilt table: {error}"
                ))
            })?;
    }
    Ok(())
}

async fn execute_rebuild_statement(
    guard: &mut RebuildConnection<'_>,
    statement: &str,
) -> Result<(), sqlx::Error> {
    if !is_foreign_key_check(statement) {
        return guard.execute(statement).await;
    }

    let rows: Vec<(String, Option<i64>, String, i64)> = sqlx::query_as(statement)
        .fetch_all(&mut **guard.connection)
        .await?;
    if let Some((table, rowid, parent, constraint)) = rows.into_iter().next() {
        let rowid = match rowid {
            Some(rowid) => rowid.to_string(),
            None => "<without rowid>".to_owned(),
        };
        return Err(sqlx::Error::Protocol(format!(
            "foreign key check failed for table {table:?}: row {rowid} references missing parent {parent:?} (constraint {constraint})"
        )));
    }
    Ok(())
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

    async fn txn_query_stream<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &'c Sql<SQLite>,
    ) -> Result<besu_core::RowStream<'c, Self>, Self::Error> {
        let arguments = arguments(sql)?;
        Ok(sqlx::query_with(sql.sql(), arguments).fetch(&mut **tx))
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

/// Decode a 128-bit integer stored in a `TEXT` column as its decimal digits.
///
/// An integer value is accepted too, so a hand-written `SELECT` that happens to yield one
/// still decodes.
fn decode_128<T>(row: &SqliteRow, offset: &mut usize, what: &str) -> Result<T, sqlx::Error>
where
    T: TryFrom<i64> + core::str::FromStr,
{
    let index = *offset;

    if let Ok(value) = row.try_get::<String, _>(index) {
        *offset += 1;
        return value.trim().parse().map_err(|_| {
            <Sqlx as Driver>::error(Error::decode(
                index,
                format!("the value is not a valid {what}"),
            ))
        });
    }

    let value: i64 = decode_via_sqlx(row, offset)?;
    T::try_from(value).map_err(|_| {
        <Sqlx as Driver>::error(Error::decode(
            index,
            format!("a SQLite integer is out of range for {what}"),
        ))
    })
}

impl Decode<Sqlx> for u128 {
    fn decode(row: &SqliteRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        decode_128(row, offset, "u128")
    }
}

impl Decode<Sqlx> for i128 {
    fn decode(row: &SqliteRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        decode_128(row, offset, "i128")
    }
}

impl Decode<Sqlx> for serde_json::Value {
    fn decode(row: &SqliteRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let value: sqlx::types::Json<serde_json::Value> = decode_via_sqlx(row, offset)?;
        Ok(value.0)
    }
}

impl Decode<Sqlx> for SqlInterval {
    fn decode(row: &SqliteRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;
        let text: String = decode_via_sqlx(row, offset)?;
        text.trim()
            .parse()
            .map_err(|err: besu_core::ParseValueError| {
                <Sqlx as Driver>::error(Error::decode(index, err.to_string()))
            })
    }
}

impl Decode<Sqlx> for Inet {
    fn decode(row: &SqliteRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;
        let text: String = decode_via_sqlx(row, offset)?;
        Self::new(text.trim())
            .map_err(|err| <Sqlx as Driver>::error(Error::decode(index, err.to_string())))
    }
}

impl Decode<Sqlx> for Cidr {
    fn decode(row: &SqliteRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;
        let text: String = decode_via_sqlx(row, offset)?;
        Self::new(text.trim())
            .map_err(|err| <Sqlx as Driver>::error(Error::decode(index, err.to_string())))
    }
}

impl Decode<Sqlx> for BitString {
    fn decode(row: &SqliteRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;
        let text: String = decode_via_sqlx(row, offset)?;
        Self::new(text.trim())
            .map_err(|err| <Sqlx as Driver>::error(Error::decode(index, err.to_string())))
    }
}

impl<T> Decode<Sqlx> for Range<T>
where
    T: core::str::FromStr,
{
    fn decode(row: &SqliteRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;
        let text: String = decode_via_sqlx(row, offset)?;
        Range::parse_with(text.trim(), str::parse::<T>)
            .map_err(|err| <Sqlx as Driver>::error(Error::decode(index, err.to_string())))
    }
}

impl Decode<Sqlx> for Geometry {
    fn decode(row: &SqliteRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;
        let bytes: Vec<u8> = decode_via_sqlx(row, offset)?;
        Self::from_ewkb(bytes)
            .map_err(|err| <Sqlx as Driver>::error(Error::decode(index, err.to_string())))
    }
}

impl Decode<Sqlx> for Vector {
    fn decode(row: &SqliteRow, offset: &mut usize) -> Result<Self, sqlx::Error> {
        let index = *offset;
        let text: String = decode_via_sqlx(row, offset)?;
        text.trim()
            .parse()
            .map_err(|err: besu_core::ParseValueError| {
                <Sqlx as Driver>::error(Error::decode(index, err.to_string()))
            })
    }
}
