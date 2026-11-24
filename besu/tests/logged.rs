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
    future::{Future, poll_fn},
    marker::PhantomData,
    pin::{Pin, pin},
    sync::{Arc, Mutex},
    task::{Context, Poll, Waker},
    time::Duration,
};

use besu::{Database, Logged, Policy, Stream};
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

fn credentials() -> Sql<TestDialect> {
    let mut sql = Sql::<TestDialect>::raw(
        "INSERT INTO user_credentials (user_id, password, api_token) VALUES (",
    );
    sql.bind(42_i64);
    sql.push_raw(", ");
    sql.bind("DUMMY_PASSWORD_NEVER_USE_7Qm4");
    sql.push_raw(", ");
    sql.bind("DUMMY_API_TOKEN_NEVER_USE_k9P2");
    sql.push_raw(")");
    sql
}

fn record(
    statements: &Mutex<Vec<String>>,
    bindings: &Mutex<Vec<Vec<Value>>>,
    sql: &Sql<TestDialect>,
) {
    statements
        .lock()
        .expect("the statement log is never poisoned")
        .push(sql.sql().to_owned());
    bindings
        .lock()
        .expect("the binding log is never poisoned")
        .push(sql.args().to_vec());
}

/// A driver that records what it was asked to run and returns a fixed result.
#[derive(Clone, Default)]
struct Fake {
    statements: Arc<Mutex<Vec<String>>>,
    bindings: Arc<Mutex<Vec<Vec<Value>>>>,
}

impl Fake {
    fn statements(&self) -> Vec<String> {
        self.statements
            .lock()
            .expect("the statement log is never poisoned")
            .clone()
    }

    fn bindings(&self) -> Vec<Vec<Value>> {
        self.bindings
            .lock()
            .expect("the binding log is never poisoned")
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
        record(&self.statements, &self.bindings, sql);
        if sql.sql().contains("FAIL") {
            return Err(Error::unsupported("the fake driver was asked to fail"));
        }
        Ok(vec![1, 2, 3])
    }

    async fn execute(&self, sql: &Sql<Self::Dialect>) -> Result<Self::Output, Self::Error> {
        record(&self.statements, &self.bindings, sql);
        Ok(1)
    }
}

struct FakeTransaction<'a> {
    statements: Arc<Mutex<Vec<String>>>,
    bindings: Arc<Mutex<Vec<Vec<Value>>>>,
    marker: PhantomData<&'a ()>,
}

impl TransactionDriver for Fake {
    type Transaction<'a> = FakeTransaction<'a>;

    async fn begin<'a>(&'a self) -> Result<Self::Transaction<'a>, Self::Error> {
        Ok(FakeTransaction {
            statements: self.statements.clone(),
            bindings: self.bindings.clone(),
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
        record(&tx.statements, &tx.bindings, sql);
        Ok(vec![1, 2, 3])
    }

    async fn txn_execute<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &Sql<Self::Dialect>,
    ) -> Result<Self::Output, Self::Error> {
        record(&tx.statements, &tx.bindings, sql);
        Ok(1)
    }

    async fn txn_query_stream<'c, 'a>(
        tx: &'c mut Self::Transaction<'a>,
        sql: &'c Sql<Self::Dialect>,
    ) -> Result<besu::RowStream<'c, Self>, Self::Error> {
        record(&tx.statements, &tx.bindings, sql);
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
    only_level: Option<Level>,
}

impl Recorder {
    fn take(&self) -> Vec<Recorded> {
        std::mem::take(&mut *self.events.lock().expect("the recorder is never poisoned"))
    }

    fn at_level(level: Level) -> Self {
        Self {
            only_level: Some(level),
            ..Self::default()
        }
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
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.target() == "besu::query"
            && self
                .only_level
                .is_none_or(|level| *metadata.level() == level)
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
        Some("SELECT * FROM users WHERE id = ?"),
        "bound values must not enter the tracing pipeline"
    );
    assert!(event.field("elapsed").is_some());
}

#[test]
fn bound_credentials_never_reach_debug_events_or_the_driver_log() {
    let driver = Fake::default();
    let db = Database::new(Logged::new(driver.clone()));
    let recorder = Recorder::default();
    let sql = credentials();
    let expected = sql.args().to_vec();

    run(&recorder, async {
        db.driver().query(&sql).await.expect("query");
        db.driver().execute(&sql).await.expect("execute");
        let mut stream = db.driver().query_stream(&sql).await.expect("stream");
        while poll_fn(|cx| Pin::new(&mut stream).poll_next(cx))
            .await
            .is_some()
        {}
    });

    assert_eq!(
        driver.bindings(),
        vec![expected.clone(), expected.clone(), expected]
    );
    let events = recorder.take();
    assert_eq!(events.len(), 3, "{events:?}");
    for event in events {
        assert_eq!(
            event.field("sql"),
            Some("INSERT INTO user_credentials (user_id, password, api_token) VALUES (?, ?, ?)")
        );
        assert_eq!(event.field("args"), Some("3"));
        assert!(
            event.fields.iter().all(
                |(_, value)| !value.contains("DUMMY_PASSWORD_NEVER_USE_7Qm4")
                    && !value.contains("DUMMY_API_TOKEN_NEVER_USE_k9P2")
            ),
            "{event:?}"
        );
    }
}

#[test]
fn bound_credentials_never_reach_warn_only_slow_events() {
    let driver = Fake::default();
    let logged = Logged::with_policy(driver.clone(), Policy::slow_only(Duration::ZERO));
    let recorder = Recorder::at_level(Level::WARN);
    let sql = credentials();
    let expected = sql.args().to_vec();

    run(&recorder, async {
        <Logged<Fake> as Driver>::query(&logged, &sql)
            .await
            .expect("query");
        let mut tx = <Logged<Fake> as TransactionDriver>::begin(&logged)
            .await
            .expect("begin");
        <Logged<Fake> as TransactionDriver>::txn_execute(&mut tx, &sql)
            .await
            .expect("transaction execute");
        <Logged<Fake> as TransactionDriver>::commit(tx)
            .await
            .expect("commit");
    });

    assert_eq!(driver.bindings(), vec![expected.clone(), expected]);
    let events = recorder.take();
    assert_eq!(events.len(), 2, "{events:?}");
    for event in events {
        assert_eq!(event.level, Level::WARN);
        assert_eq!(event.field("args"), Some("3"));
        assert_eq!(
            event.field("sql"),
            Some("INSERT INTO user_credentials (user_id, password, api_token) VALUES (?, ?, ?)")
        );
        assert!(
            event.fields.iter().all(
                |(_, value)| !value.contains("DUMMY_PASSWORD_NEVER_USE_7Qm4")
                    && !value.contains("DUMMY_API_TOKEN_NEVER_USE_k9P2")
            ),
            "{event:?}"
        );
    }
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
    assert_eq!(event.field("sql"), Some("SELECT * FROM users WHERE id = ?"));
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

/// Wrapping a prefixed database used to build a fresh `Database` around the logged driver,
/// which dropped the tenant prefix: every table name rendered unprefixed afterwards.
#[test]
fn wrapping_in_place_keeps_the_table_prefix() {
    let db = Database::prefixed(Fake::default(), "tenant_42_").logged(Policy::new());
    assert_eq!(db.prefix(), Some("tenant_42_"));
}

/// A statement's comment is part of its text, so the event that reports the statement
/// reports the label too — no extra field, and no way for the two to disagree.
#[cfg(feature = "derive")]
#[allow(unused_imports)]
mod comment {
    use super::*;
    use besu::prelude::*;

    #[derive(Table)]
    #[besu(table = "users")]
    pub struct User {
        #[besu(primary_key)]
        pub id: i64,
    }

    #[test]
    fn a_comment_reaches_the_log() {
        let driver = Fake::default();
        let db = Database::new(Logged::new(driver.clone()));
        let recorder = Recorder::default();

        let sql = db
            .select(users)
            .pick(users.id)
            .comment("feed page")
            .to_sql()
            .expect("a select renders");

        run(&recorder, db.driver().query(&sql)).expect("the fake driver never fails");

        let events = recorder.take();
        assert_eq!(events.len(), 1, "{events:?}");
        let reported = events[0].field("sql").expect("the event reports the SQL");
        assert!(
            reported.ends_with("/* feed page */"),
            "the comment is not in the log: {reported}"
        );
    }
}
