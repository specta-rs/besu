//! Logging and timing for every statement a driver runs.
//!
//! [`Logged`] wraps a driver and reports each `query` and `execute` through
//! [`tracing`], along with how long it took. That is the whole feature: there is no
//! separate instrumentation to switch on, so every statement that goes through the
//! wrapper is reported, including the ones the query builders generate and the ones
//! migrations run.
//!
//! ```ignore
//! use std::time::Duration;
//! use besu::{Database, Logged, Policy};
//!
//! let policy = Policy::new().slow(Duration::from_millis(100));
//! let db = Database::new(Logged::with_policy(driver, policy));
//!
//! // Or wrap an existing database, which clones the driver handle.
//! let db = db.logged(Policy::new().slow(Duration::from_millis(100)));
//! ```
//!
//! Statements are reported as *events*, not spans, because the useful fields are only
//! known after the statement finishes. A span covering the wait would need its SQL before
//! the statement ran, and a slow statement is exactly the thing you find out about
//! afterwards.
//!
//! # What it costs
//!
//! Nothing when nobody is listening. Every event is guarded by
//! [`tracing::enabled!`], so with no subscriber the wrapper performs two atomic loads per
//! statement, never touches the clock and never allocates the rendered SQL. With the
//! `tracing` feature off the module does not exist at all.
//!
//! When it *is* listening, the SQL is rendered with [`Sql::inspect`], which inlines the
//! bound parameters. That output is for humans and debugging only: it is not quoted for
//! every dialect, it is not guaranteed to be valid SQL, and it must never be executed.
//! The statement that actually runs is always `sql.sql()` with `sql.args()` bound, so
//! logging can never change what the database sees.
//!
//! # The policy
//!
//! [`Policy`] decides what gets reported:
//!
//! - [`Policy::new`] logs every statement at `DEBUG`, which is where per-statement detail
//!   belongs: one line per insert swamps `INFO`.
//! - [`Policy::slow`] adds a `WARN` when a statement takes at least that long. The
//!   threshold is checked independently of the `DEBUG` log, so you can turn the firehose
//!   off and keep the slow-query hook.
//! - [`Policy::quiet`] turns the per-statement log off and leaves only the slow-query
//!   warning.
//!
//! All events use the target `besu::query`, so `RUST_LOG=besu::query=debug` is enough to
//! see them.
//!
//! A statement is reported whether it succeeded or failed, and its result is returned
//! untouched: the wrapper never turns an error into a log line, or the other way around.
//!
//! # Timing is wall clock
//!
//! The measurement is the time from handing the statement to the driver to getting the
//! result back. On a pool that includes waiting for a connection, and on a busy server it
//! includes time spent waiting to be scheduled. That is the number a user experiences,
//! which is the one worth logging, but it is not the database's own execution time. Use
//! the database's `EXPLAIN` for that.

use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use std::time::Instant;

use alloc::string::String;

use besu_core::{Dialect, Driver, Error, RowStream, Sql, Stream, TransactionDriver};

use crate::Database;
#[cfg(feature = "migrate")]
use crate::migrate::SchemaDriver;

/// The [`tracing`] target every event from [`Logged`] uses.
const TARGET: &str = "besu::query";

/// A driver wrapper that logs and times every statement.
///
/// See the [module documentation](self) for what it emits and what it costs. The wrapper
/// is transparent: [`Dialect`], `Row` and `Output` are the inner driver's, so it slots in
/// wherever the driver went before.
///
/// [`Dialect`]: besu_core::Driver::Dialect
pub struct Logged<Dr> {
    inner: Dr,
    policy: Policy,
}

impl<Dr> Logged<Dr> {
    /// Wrap `driver`, logging every statement at `DEBUG`.
    pub fn new(driver: Dr) -> Self {
        Self::with_policy(driver, Policy::new())
    }

    /// Wrap `driver` with an explicit [`Policy`].
    pub fn with_policy(driver: Dr, policy: Policy) -> Self {
        Self {
            inner: driver,
            policy,
        }
    }

    /// The driver being wrapped.
    pub fn inner(&self) -> &Dr {
        &self.inner
    }

    /// Unwrap the driver. Use this when you are done with logging and want the concrete
    /// driver type back, for example to hand it to another wrapper.
    pub fn into_inner(self) -> Dr {
        self.inner
    }

    /// The policy in use.
    pub fn policy(&self) -> &Policy {
        &self.policy
    }
}

impl<Dr: Clone> Clone for Logged<Dr> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            policy: self.policy,
        }
    }
}

impl<Dr: core::fmt::Debug> core::fmt::Debug for Logged<Dr> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Logged")
            .field("inner", &self.inner)
            .field("policy", &self.policy)
            .finish()
    }
}

/// What [`Logged`] reports.
///
/// A policy is `Copy`: it is two words and a flag, and every transaction carries one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    slow: Option<Duration>,
    log_queries: bool,
}

impl Policy {
    /// Log every statement at `DEBUG`.
    ///
    /// This is the default: per-statement detail is useful in development and is one
    /// filter away from being silenced in production.
    pub const fn new() -> Self {
        Self {
            slow: None,
            log_queries: true,
        }
    }

    /// Log only statements that take at least `threshold`, and nothing else.
    pub const fn slow_only(threshold: Duration) -> Self {
        Self {
            slow: Some(threshold),
            log_queries: false,
        }
    }

    /// Turn the per-statement `DEBUG` log off, keeping the slow-query warning.
    pub const fn quiet(mut self) -> Self {
        self.log_queries = false;
        self
    }

    /// Change the slow-query threshold.
    pub const fn slow(mut self, threshold: Duration) -> Self {
        self.slow = Some(threshold);
        self
    }

    /// Remove the slow-query threshold.
    pub const fn no_slow(mut self) -> Self {
        self.slow = None;
        self
    }

    /// Turn the per-statement `DEBUG` log on or off.
    pub const fn log_queries(mut self, enabled: bool) -> Self {
        self.log_queries = enabled;
        self
    }

    /// The slow-query threshold, if one is set.
    pub const fn slow_threshold(&self) -> Option<Duration> {
        self.slow
    }

    /// Whether every statement is logged at `DEBUG`.
    pub const fn logs_queries(&self) -> bool {
        self.log_queries
    }

    /// Whether anything could be reported for a statement that has not finished yet.
    ///
    /// Checking this before taking a timestamp keeps the wrapper free for callers who
    /// have no subscriber: a policy that logs nothing, with tracing switched off, does
    /// not even read the clock.
    fn reporting(&self) -> bool {
        (self.log_queries && tracing::enabled!(target: TARGET, tracing::Level::DEBUG))
            || (self.slow.is_some() && tracing::enabled!(target: TARGET, tracing::Level::WARN))
    }

    /// Report a stream that has finished, from the SQL text captured when it started.
    fn report_text(&self, kind: &'static str, sql: &str, elapsed: Duration) {
        if self.log_queries && tracing::enabled!(target: TARGET, tracing::Level::DEBUG) {
            tracing::debug!(target: TARGET, kind, elapsed = ?elapsed, sql, "statement");
        }

        if let Some(threshold) = self.slow {
            if elapsed >= threshold && tracing::enabled!(target: TARGET, tracing::Level::WARN) {
                tracing::warn!(
                    target: TARGET,
                    kind,
                    elapsed = ?elapsed,
                    threshold = ?threshold,
                    sql,
                    "slow statement"
                );
            }
        }
    }

    /// Report one finished statement.
    fn report<D: Dialect>(&self, kind: &'static str, sql: &Sql<D>, elapsed: Duration) {
        if self.log_queries && tracing::enabled!(target: TARGET, tracing::Level::DEBUG) {
            tracing::debug!(
                target: TARGET,
                kind,
                args = sql.args().len(),
                elapsed = ?elapsed,
                sql = %sql.inspect(),
                "statement"
            );
        }

        if let Some(threshold) = self.slow {
            if elapsed >= threshold && tracing::enabled!(target: TARGET, tracing::Level::WARN) {
                tracing::warn!(
                    target: TARGET,
                    kind,
                    args = sql.args().len(),
                    elapsed = ?elapsed,
                    threshold = ?threshold,
                    sql = %sql.inspect(),
                    "slow statement"
                );
            }
        }
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self::new()
    }
}

/// Wrap a driver in [`Logged`] with the default [`Policy`].
///
/// ```ignore
/// let db = besu::Database::new(besu::logged(driver));
/// ```
pub fn logged<Dr>(driver: Dr) -> Logged<Dr> {
    Logged::new(driver)
}

impl<Dr: Driver> Database<Dr> {
    /// Wrap this database's driver in [`Logged`] with `policy`.
    ///
    /// The driver handle is cloned, which for every driver shipped with Besu is a cheap
    /// pointer clone of a pool. The returned database has exactly the same dialect.
    pub fn logged(&self, policy: Policy) -> Database<Logged<Dr>> {
        Database::new(Logged::with_policy(self.driver().clone(), policy))
    }
}

impl<Dr: Driver> Driver for Logged<Dr> {
    type Dialect = Dr::Dialect;
    type Error = Dr::Error;
    type Row = Dr::Row;
    type Output = Dr::Output;

    fn error(err: Error) -> Self::Error {
        Dr::error(err)
    }

    fn row_len(row: &Self::Row) -> usize {
        Dr::row_len(row)
    }

    fn is_null(row: &Self::Row, index: usize) -> bool {
        Dr::is_null(row, index)
    }

    async fn query(&self, sql: &Sql<Dr::Dialect>) -> Result<Vec<Self::Row>, Self::Error> {
        let started = self.policy.reporting().then(Instant::now);
        let result = self.inner.query(sql).await;
        if let Some(started) = started {
            self.policy.report("query", sql, started.elapsed());
        }
        result
    }

    async fn execute(&self, sql: &Sql<Dr::Dialect>) -> Result<Self::Output, Self::Error> {
        let started = self.policy.reporting().then(Instant::now);
        let result = self.inner.execute(sql).await;
        if let Some(started) = started {
            self.policy.report("execute", sql, started.elapsed());
        }
        result
    }

    // The `'a` bound is stronger than the trait's, which is deliberate: `Logged` never
    // shortens a stream's borrow, and stating that lets callers keep the stream after the
    // future has been awaited.
    #[allow(refining_impl_trait)]
    fn query_stream<'a>(
        &'a self,
        sql: &'a Sql<Dr::Dialect>,
    ) -> impl Future<Output = Result<RowStream<'a, Dr>, Self::Error>> + Send + 'a {
        // A cursor outlives this call, so the event is emitted when the stream ends rather
        // than around it. That is the only honest place to measure it.
        let started = self.policy.reporting().then(Instant::now);
        let summary = started.map(|_| alloc::string::ToString::to_string(&sql.inspect()));
        let policy = self.policy;
        let inner = self.inner.query_stream(sql);

        async move {
            let inner = inner.await?;
            match started {
                Some(started) => Ok(Box::pin(LoggedStream::<Dr> {
                    inner,
                    started,
                    policy,
                    summary: summary.unwrap_or_default(),
                }) as RowStream<'a, Dr>),
                None => Ok(inner),
            }
        }
    }
}

/// A row stream that reports when it finishes.
struct LoggedStream<'a, Dr: Driver> {
    inner: RowStream<'a, Dr>,
    started: Instant,
    policy: Policy,
    summary: String,
}

impl<Dr: Driver> Stream for LoggedStream<'_, Dr> {
    type Item = Result<Dr::Row, Dr::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();
        let next = Pin::new(&mut this.inner).poll_next(cx);
        if let Poll::Ready(None) = next {
            this.policy
                .report_text("stream", &this.summary, this.started.elapsed());
        }
        next
    }
}

/// The transaction handle of a [`Logged`] driver.
///
/// It exists so a statement run inside a transaction goes through the same [`Policy`] as
/// a statement run on the database. You never name it: [`Database::begin`] and
/// [`Database::transaction`] hand one back through the ordinary `Transaction` wrapper.
pub struct LoggedTransaction<'a, Dr: TransactionDriver> {
    inner: Dr::Transaction<'a>,
    policy: Policy,
}

impl<'a, Dr: TransactionDriver> LoggedTransaction<'a, Dr> {
    /// The inner transaction handle.
    pub fn inner(&mut self) -> &mut Dr::Transaction<'a> {
        &mut self.inner
    }

    /// Unwrap the inner transaction handle.
    pub fn into_inner(self) -> Dr::Transaction<'a> {
        self.inner
    }
}

impl<Dr: TransactionDriver> TransactionDriver for Logged<Dr> {
    type Transaction<'a>
        = LoggedTransaction<'a, Dr>
    where
        Self: 'a;

    async fn begin<'a>(&'a self) -> Result<Self::Transaction<'a>, Self::Error> {
        Ok(LoggedTransaction {
            inner: self.inner.begin().await?,
            policy: self.policy,
        })
    }

    async fn commit<'a>(tx: Self::Transaction<'a>) -> Result<(), Self::Error> {
        Dr::commit(tx.inner).await
    }

    async fn rollback<'a>(tx: Self::Transaction<'a>) -> Result<(), Self::Error> {
        Dr::rollback(tx.inner).await
    }

    async fn txn_query<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &Sql<Self::Dialect>,
    ) -> Result<Vec<Self::Row>, Self::Error> {
        let started = tx.policy.reporting().then(Instant::now);
        let result = Dr::txn_query(&mut tx.inner, sql).await;
        if let Some(started) = started {
            tx.policy.report("query", sql, started.elapsed());
        }
        result
    }

    async fn txn_execute<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &Sql<Self::Dialect>,
    ) -> Result<Self::Output, Self::Error> {
        let started = tx.policy.reporting().then(Instant::now);
        let result = Dr::txn_execute(&mut tx.inner, sql).await;
        if let Some(started) = started {
            tx.policy.report("execute", sql, started.elapsed());
        }
        result
    }

    async fn txn_query_stream<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &'c Sql<Self::Dialect>,
    ) -> Result<RowStream<'c, Self>, Self::Error> {
        let started = tx.policy.reporting().then(Instant::now);
        let summary = started.map(|_| alloc::string::ToString::to_string(&sql.inspect()));
        let policy = tx.policy;
        let inner = Dr::txn_query_stream(&mut tx.inner, sql).await?;
        match started {
            Some(started) => Ok(Box::pin(LoggedStream::<Dr> {
                inner,
                started,
                policy,
                summary: summary.unwrap_or_default(),
            }) as RowStream<'c, Dr>),
            None => Ok(inner),
        }
    }
}

#[cfg(feature = "migrate")]
impl<Dr: SchemaDriver> SchemaDriver for Logged<Dr> {
    type Ddl = <Dr as SchemaDriver>::Ddl;

    async fn introspect(&self) -> Result<crate::schema::Schema, Self::Error> {
        // Introspection is a batch of driver queries, and each one is logged by the
        // wrapped driver; a second event here would only duplicate them.
        self.inner.introspect().await
    }
}
