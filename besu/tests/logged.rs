//! Tests for the `tracing` feature's [`Logged`] driver wrapper.
//!
//! `besu-sqlite` depends on `besu`, so a test in this crate cannot use it without a
//! dependency cycle; the fake driver below stands in for a real one. That is enough here,
//! because the wrapper only delegates: what these tests pin down is that the statement
//! reaches the driver unchanged and exactly once, and that the events describe it.
#![cfg(feature = "tracing")]
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use std::{
    borrow::Cow,
    fmt,
    future::Future,
    marker::PhantomData,
    pin::pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, Waker},
    time::Duration,
};

use besu::{Database, Logged, Policy};
use besu_core::{
    Dialect, Driver, Error, Placeholder, Sql, TransactionDriver, TypeDef, UpsertStyle, Value,
};
use tracing::{
    Event, Level, Metadata, Subscriber,
    field::{Field, Visit},
    span,
};

// ---------------------------------------------------------------------------------------
// A dialect and driver with no database behind them
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TestDialect;

impl Dialect for TestDialect {
    type Value = Value;

    const NAME: &'static str = "test";
    const PLACEHOLDER: Placeholder = Placeholder::Question;
    const SUPPORTS_RETURNING: bool = true;
    const SUPPORTS_ILIKE: bool = false;
    const SUPPORTS_FULL_JOIN: bool = true;
    const UPSERT: UpsertStyle = UpsertStyle::OnConflict;

    fn bind(value: Value) -> Result<Self::Value, Error> {
        Ok(value)
    }

    fn write_placeholder(out: &mut String, _index: usize) {
        out.push('?');
    }

    fn type_name(_ty: TypeDef) -> Cow<'static, str> {
        Cow::Borrowed("TEXT")
    }
}

/// A statement with exactly one bound parameter, so the argument count has something to
/// report.
fn find_user(id: i64) -> Sql<TestDialect> {
    let mut sql = Sql::<TestDialect>::raw("SELECT * FROM users WHERE id = ");
    sql.bind(id);
    sql
}

fn record(statements: &Mutex<Vec<String>>, sql: &Sql<TestDialect>) {
    statements
        .lock()
        .expect("the statement log is never poisoned")
        .push(sql.sql().to_owned());
}

/// A driver that records what it was asked to run and returns a fixed result.
#[derive(Clone, Default)]
struct Fake {
    statements: Arc<Mutex<Vec<String>>>,
}

impl Fake {
    fn statements(&self) -> Vec<String> {
        self.statements
            .lock()
            .expect("the statement log is never poisoned")
            .clone()
    }
}

impl Driver for Fake {
    type Dialect = TestDialect;
    type Error = Error;
    type Row = i64;
    type Output = usize;

    fn error(err: Error) -> Self::Error {
        err
    }

    fn row_len(_row: &Self::Row) -> usize {
        1
    }

    fn is_null(_row: &Self::Row, _index: usize) -> bool {
        false
    }

    async fn query(&self, sql: &Sql<Self::Dialect>) -> Result<Vec<Self::Row>, Self::Error> {
        record(&self.statements, sql);
        if sql.sql().contains("FAIL") {
            return Err(Error::unsupported("the fake driver was asked to fail"));
        }
        Ok(vec![1, 2, 3])
    }

    async fn execute(&self, sql: &Sql<Self::Dialect>) -> Result<Self::Output, Self::Error> {
        record(&self.statements, sql);
        Ok(1)
    }
}

struct FakeTransaction<'a> {
    statements: Arc<Mutex<Vec<String>>>,
    marker: PhantomData<&'a ()>,
}

impl TransactionDriver for Fake {
    type Transaction<'a> = FakeTransaction<'a>;

    async fn begin<'a>(&'a self) -> Result<Self::Transaction<'a>, Self::Error> {
        Ok(FakeTransaction {
            statements: self.statements.clone(),
            marker: PhantomData,
        })
    }

    async fn commit<'a>(_tx: Self::Transaction<'a>) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn rollback<'a>(_tx: Self::Transaction<'a>) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn txn_query<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &Sql<Self::Dialect>,
    ) -> Result<Vec<Self::Row>, Self::Error> {
        record(&tx.statements, sql);
        Ok(vec![1, 2, 3])
    }

    async fn txn_execute<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &Sql<Self::Dialect>,
    ) -> Result<Self::Output, Self::Error> {
        record(&tx.statements, sql);
        Ok(1)
    }

    async fn txn_query_stream<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &'c Sql<Self::Dialect>,
    ) -> Result<besu::RowStream<'c, Self>, Self::Error> {
        record(&tx.statements, sql);
        let rows: Vec<Result<i64, Error>> = vec![Ok(1), Ok(2), Ok(3)];
        Ok(Box::pin(besu::futures_stream::iter(rows)))
    }
}

// ---------------------------------------------------------------------------------------
// A subscriber that keeps events in memory
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct Recorded {
    level: Level,
    target: String,
    fields: Vec<(String, String)>,
}

impl Recorded {
    fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Clone, Default)]
struct Recorder {
    events: Arc<Mutex<Vec<Recorded>>>,
}

impl Recorder {
    fn take(&self) -> Vec<Recorded> {
        std::mem::take(&mut *self.events.lock().expect("the recorder is never poisoned"))
    }
}

#[derive(Default)]
struct Fields(Vec<(String, String)>);

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.push((field.name().to_owned(), value.to_owned()));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0.push((field.name().to_owned(), format!("{value:?}")));
    }
}

impl Subscriber for Recorder {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _attributes: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(1)
    }

    fn record(&self, _span: &span::Id, _values: &span::Record<'_>) {}

    fn record_follows_from(&self, _span: &span::Id, _follows: &span::Id) {}

    fn enter(&self, _span: &span::Id) {}

    fn exit(&self, _span: &span::Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        self.events
            .lock()
            .expect("the recorder is never poisoned")
            .push(Recorded {
                level: *event.metadata().level(),
                target: event.metadata().target().to_owned(),
                fields: fields.0,
            });
    }
}

/// Run a future to completion. The fake driver's futures are always ready, so a no-op
/// waker is all that is needed and the tests do not pull in a runtime.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

fn run<T>(recorder: &Recorder, future: impl Future<Output = T>) -> T {
    tracing::subscriber::with_default(recorder.clone(), || block_on(future))
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

#[test]
fn a_query_still_returns_the_right_rows_and_runs_once() {
    let driver = Fake::default();
    let db = Database::new(Logged::new(driver.clone()));
    let recorder = Recorder::default();

    let rows =
        run(&recorder, db.driver().query(&find_user(42))).expect("the fake driver never fails");

    assert_eq!(rows, vec![1, 2, 3]);
    assert_eq!(
        driver.statements(),
        vec!["SELECT * FROM users WHERE id = ?"],
        "the wrapper must not rewrite the statement"
    );

    let events = recorder.take();
    assert_eq!(events.len(), 1, "one statement, one event: {events:?}");

    let event = &events[0];
    assert_eq!(event.level, Level::DEBUG);
    assert_eq!(event.target, "besu::query");
    assert_eq!(event.field("kind"), Some("query"));
    assert_eq!(event.field("args"), Some("1"));
    assert_eq!(event.field("message"), Some("statement"));
    assert_eq!(
        event.field("sql"),
        Some("SELECT * FROM users WHERE id = 42"),
        "parameters are inlined for humans"
    );
    assert!(event.field("elapsed").is_some());
}

#[test]
fn execute_is_logged_with_its_result() {
    let driver = Fake::default();
    let db = Database::new(Logged::new(driver.clone()));
    let recorder = Recorder::default();

    let affected =
        run(&recorder, db.driver().execute(&find_user(7))).expect("the fake driver never fails");

    assert_eq!(affected, 1);
    assert_eq!(driver.statements().len(), 1, "exactly one execution");

    let events = recorder.take();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].field("kind"), Some("execute"));
    assert_eq!(events[0].field("args"), Some("1"));
}

#[test]
fn a_zero_threshold_turns_every_statement_into_a_warning() {
    let driver = Fake::default();
    let policy = Policy::new().slow(Duration::ZERO).quiet();
    let db = Database::new(Logged::with_policy(driver.clone(), policy));
    let recorder = Recorder::default();

    run(&recorder, db.driver().query(&find_user(3))).expect("the fake driver never fails");

    let events = recorder.take();
    assert_eq!(
        events.len(),
        1,
        "a quiet policy must not also emit the DEBUG event: {events:?}"
    );

    let event = &events[0];
    assert_eq!(event.level, Level::WARN);
    assert_eq!(event.target, "besu::query");
    assert_eq!(event.field("kind"), Some("query"));
    assert_eq!(event.field("message"), Some("slow statement"));
    assert_eq!(event.field("args"), Some("1"));
    assert_eq!(event.field("threshold"), Some("0ns"));
    assert_eq!(event.field("sql"), Some("SELECT * FROM users WHERE id = 3"));
}

#[test]
fn a_statement_under_the_threshold_does_not_warn() {
    let driver = Fake::default();
    let policy = Policy::new().slow(Duration::from_secs(3600));
    let db = Database::new(Logged::with_policy(driver, policy));
    let recorder = Recorder::default();

    run(&recorder, db.driver().query(&find_user(3))).expect("the fake driver never fails");

    let events = recorder.take();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].level, Level::DEBUG);
    assert!(events.iter().all(|event| event.level != Level::WARN));
}

#[test]
fn a_quiet_policy_with_no_threshold_logs_nothing() {
    let driver = Fake::default();
    let db = Database::new(Logged::with_policy(driver, Policy::new().quiet()));
    let recorder = Recorder::default();

    run(&recorder, db.driver().execute(&find_user(3))).expect("the fake driver never fails");

    assert!(recorder.take().is_empty());
}

#[test]
fn statements_inside_a_transaction_are_logged() {
    let driver = Fake::default();
    let logged = Logged::new(driver.clone());
    let recorder = Recorder::default();

    let (rows, affected) = run(&recorder, async {
        let mut tx = <Logged<Fake> as TransactionDriver>::begin(&logged)
            .await
            .expect("begin");
        let rows = <Logged<Fake> as TransactionDriver>::txn_query(&mut tx, &find_user(11))
            .await
            .expect("txn_query");
        let affected = <Logged<Fake> as TransactionDriver>::txn_execute(&mut tx, &find_user(12))
            .await
            .expect("txn_execute");
        <Logged<Fake> as TransactionDriver>::commit(tx)
            .await
            .expect("commit");
        (rows, affected)
    });

    assert_eq!(rows, vec![1, 2, 3]);
    assert_eq!(affected, 1);
    assert_eq!(
        driver.statements(),
        vec![
            "SELECT * FROM users WHERE id = ?",
            "SELECT * FROM users WHERE id = ?",
        ],
        "one execution per statement, not one per log line"
    );

    let events = recorder.take();
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[0].field("kind"), Some("query"));
    assert_eq!(events[0].field("args"), Some("1"));
    assert_eq!(events[1].field("kind"), Some("execute"));
    assert!(events.iter().all(|event| event.level == Level::DEBUG));
}

#[test]
fn a_failing_statement_is_reported_and_the_error_is_unchanged() {
    let driver = Fake::default();
    let db = Database::new(Logged::new(driver.clone()));
    let recorder = Recorder::default();

    let err = run(
        &recorder,
        db.driver().query(&Sql::<TestDialect>::raw("SELECT FAIL")),
    )
    .expect_err("the fake driver fails on FAIL");

    assert_eq!(
        err.to_string(),
        "unsupported: the fake driver was asked to fail"
    );
    assert_eq!(driver.statements().len(), 1, "exactly one execution");

    let events = recorder.take();
    assert_eq!(
        events.len(),
        1,
        "a failed statement is still reported: {events:?}"
    );
    assert_eq!(events[0].level, Level::DEBUG);
}

#[test]
fn the_database_can_be_wrapped_in_place() {
    let driver = Fake::default();
    let db = Database::new(driver.clone()).logged(Policy::new());
    let recorder = Recorder::default();

    run(&recorder, db.driver().query(&find_user(5))).expect("the fake driver never fails");

    assert_eq!(driver.statements().len(), 1);
    assert_eq!(recorder.take().len(), 1);
}
