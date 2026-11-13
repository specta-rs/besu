//! The same select and insert as the browser test, run on every native platform.
//!
//! `MemoryTransport` answers from a closure, so the test exercises the whole client path —
//! rendering, encoding, transport, decoding — with no server. The second half connects a
//! `Bridge` to `handle_request` through a fake driver, which proves the server helper really
//! runs a statement rather than manufacturing rows.

#![cfg(not(target_arch = "wasm32"))]
#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_arguments
)]

use std::{
    future::{Future, IntoFuture},
    pin::pin,
    sync::{Arc, Mutex, MutexGuard},
    task::{Context, Poll, Waker},
};

use besu::prelude::*;
use besu::{Driver, Sql};
use besu_bridge::{
    Bridge, BridgeArray, BridgeBound, BridgeDialect, BridgeError, BridgeRange, BridgeSource,
    BridgeValue, MemoryTransport, Request, RequestKind, Response, Transport, handle_request,
};
use besu_core::{Range, TypeDef};
use besu_postgres::Postgres;

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "users")]
pub struct User {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    #[besu(unique)]
    pub email: String,
    pub name: String,
    pub age: i32,
    pub bio: Option<String>,
}

fn columns() -> Vec<String> {
    ["id", "email", "name", "age", "bio"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

fn oscar() -> User {
    User {
        id: 1,
        email: "oscar@example.com".to_owned(),
        name: "Oscar".to_owned(),
        age: 21,
        bio: None,
    }
}

fn oscar_row() -> Vec<BridgeValue> {
    vec![
        BridgeValue::Int(1),
        BridgeValue::Text("oscar@example.com".to_owned()),
        BridgeValue::Text("Oscar".to_owned()),
        BridgeValue::Int(21),
        BridgeValue::Null,
    ]
}

/// A tiny executor for a future that never waits.
///
/// Every transport in these tests answers on the first poll, so a real runtime would be a
/// dependency for no reason. `Waker::noop` is stable, which keeps this free of `unsafe`.
fn block_on<F: IntoFuture>(future: F) -> F::Output {
    let mut future = pin!(future.into_future());
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

#[test]
fn select_decodes_a_typed_row() {
    let transport = MemoryTransport::new(|request| {
        assert_eq!(request.kind, RequestKind::Query);
        // The bridge emits the dialect's own spelling, so the server can run it directly.
        assert!(
            request.sql.contains("$1"),
            "expected a PostgreSQL placeholder in `{}`",
            request.sql
        );
        assert_eq!(request.params, vec![BridgeValue::Int(1)]);
        Ok(Response::rows(Some(columns()), vec![oscar_row()]))
    });
    let db = besu::Database::new(Bridge::<_, Postgres>::new(transport.clone()));

    let rows: Vec<User> = block_on(db.select(users).filter(users.id.eq(1))).unwrap();
    assert_eq!(rows, vec![oscar()]);

    let sent = transport.last_request().expect("a request was answered");
    assert!(sent.sql.starts_with("SELECT"));
    assert!(sent.sql.contains("\"users\".\"email\""));
}

#[test]
fn insert_reports_the_affected_count() {
    let transport = MemoryTransport::new(|request| {
        assert_eq!(request.kind, RequestKind::Execute);
        assert_eq!(
            request.params,
            vec![
                BridgeValue::Text("oscar@example.com".to_owned()),
                BridgeValue::Text("Oscar".to_owned()),
                BridgeValue::Int(21),
            ]
        );
        Ok(Response::affected(1))
    });
    let db = besu::Database::new(Bridge::<_, Postgres>::new(transport.clone()));

    let affected: u64 = block_on(db.insert(users).value(users.insert(
        "oscar@example.com",
        "Oscar",
        21,
    )))
    .unwrap();
    assert_eq!(affected, 1);

    let sent = transport.last_request().expect("a request was answered");
    assert!(sent.sql.starts_with("INSERT INTO \"users\""));
}

#[test]
fn insert_returning_decodes_the_written_row() {
    let transport = MemoryTransport::new(|request| {
        assert_eq!(request.kind, RequestKind::Query);
        assert!(request.sql.contains("RETURNING"));
        Ok(Response::rows(Some(columns()), vec![oscar_row()]))
    });
    let db = besu::Database::new(Bridge::<_, Postgres>::new(transport));

    let created: Vec<User> = block_on(
        db.insert(users)
            .value(users.insert("oscar@example.com", "Oscar", 21))
            .returning(users),
    )
    .unwrap();
    assert_eq!(created, vec![oscar()]);
}

#[test]
fn a_server_error_reaches_the_caller() {
    let transport = MemoryTransport::new(|_| {
        Ok(Response::error(
            "duplicate key value violates unique constraint \"users_email_key\"",
        ))
    });
    let db = besu::Database::new(Bridge::<_, Postgres>::new(transport));

    let error = block_on(db.select(users)).unwrap_err();
    assert!(matches!(error, BridgeError::Server(_)), "{error}");
    assert!(error.to_string().contains("duplicate key"));
}

#[test]
fn arrays_and_ranges_decode_into_their_rust_types() {
    let transport = MemoryTransport::new(|_| {
        Ok(Response::rows(
            Some(vec![
                "ids".to_owned(),
                "window".to_owned(),
                "tags".to_owned(),
            ]),
            vec![vec![
                BridgeValue::Array(BridgeArray {
                    element: TypeDef::Int4,
                    values: vec![BridgeValue::Int(1), BridgeValue::Int(2)],
                }),
                BridgeValue::Range(Box::new(BridgeRange::new(
                    TypeDef::Int4,
                    Some((
                        BridgeBound::Included(BridgeValue::Int(1)),
                        BridgeBound::Excluded(BridgeValue::Int(5)),
                    )),
                ))),
                BridgeValue::Array(BridgeArray {
                    element: TypeDef::Text,
                    values: vec![BridgeValue::Text("a".to_owned())],
                }),
            ]],
        ))
    });
    let db = besu::Database::new(Bridge::<_, Postgres>::new(transport));

    let rows: Vec<(Vec<i32>, Range<i64>, Vec<String>)> =
        block_on(db.execute(besu::sql!("SELECT ids, window, tags")).all()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, vec![1, 2]);
    assert_eq!(rows[0].1, Range::closed_open(1, 5));
    assert_eq!(rows[0].2, vec!["a".to_owned()]);
}

#[test]
fn decode_rejects_the_wrong_wire_type() {
    let transport = MemoryTransport::new(|_| {
        Ok(Response::rows(
            Some(columns()),
            vec![vec![
                BridgeValue::Text("one".to_owned()),
                BridgeValue::Text("oscar@example.com".to_owned()),
                BridgeValue::Text("Oscar".to_owned()),
                BridgeValue::Int(21),
                BridgeValue::Null,
            ]],
        ))
    });
    let db = besu::Database::new(Bridge::<_, Postgres>::new(transport));

    let error = block_on(db.select(users)).unwrap_err();
    assert!(
        error.to_string().contains("cannot decode column 0"),
        "{error}"
    );
}

// ---------------------------------------------------------------------------------------
// The server half, end to end
// ---------------------------------------------------------------------------------------

/// What the fake server's driver returns: rows already in wire form.
#[derive(Clone, Debug)]
struct FakeRow {
    columns: Vec<String>,
    values: Vec<BridgeValue>,
}

/// The statements the fake driver ran, with their bound values.
type Recorded = Arc<Mutex<Vec<(String, Vec<Value>)>>>;

#[derive(Clone)]
struct FakeDriver {
    rows: Arc<Vec<FakeRow>>,
    affected: u64,
    recorded: Recorded,
}

impl FakeDriver {
    fn new(rows: Vec<FakeRow>) -> Self {
        Self {
            rows: Arc::new(rows),
            affected: 1,
            recorded: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn recorded(&self) -> MutexGuard<'_, Vec<(String, Vec<Value>)>> {
        self.recorded
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Driver for FakeDriver {
    type Dialect = Postgres;
    type Error = BridgeError;
    type Row = FakeRow;
    type Output = u64;

    fn error(error: besu_core::Error) -> Self::Error {
        error.into()
    }

    fn row_len(row: &Self::Row) -> usize {
        row.values.len()
    }

    fn is_null(row: &Self::Row, index: usize) -> bool {
        row.values.get(index).is_none_or(BridgeValue::is_null)
    }

    fn column_names(row: &Self::Row) -> Option<Vec<String>> {
        Some(row.columns.clone())
    }

    async fn query(&self, sql: &Sql<Postgres>) -> Result<Vec<Self::Row>, Self::Error> {
        self.recorded()
            .push((sql.sql().to_owned(), sql.args().to_vec()));
        Ok((*self.rows).clone())
    }

    async fn execute(&self, sql: &Sql<Postgres>) -> Result<Self::Output, Self::Error> {
        self.recorded()
            .push((sql.sql().to_owned(), sql.args().to_vec()));
        Ok(self.affected)
    }
}

/// The server's answer to "how do I read my driver's rows?".
#[derive(Clone)]
struct FakeSource;

impl BridgeSource<FakeDriver> for FakeSource {
    fn encode_row(&self, row: &FakeRow) -> Result<Vec<BridgeValue>, BridgeError> {
        Ok(row.values.clone())
    }

    fn affected(&self, output: &u64) -> u64 {
        *output
    }
}

/// A transport that runs requests against a driver instead of posting them anywhere.
#[derive(Clone)]
struct Loopback {
    driver: FakeDriver,
    source: FakeSource,
}

impl Transport for Loopback {
    fn send(&self, request: Request) -> impl Future<Output = Result<Response, BridgeError>> + Send {
        let this = self.clone();
        async move { Ok(handle_request(&this.driver, &this.source, request).await) }
    }
}

#[test]
fn the_server_helper_runs_the_statement_and_serialises_its_rows() {
    let driver = FakeDriver::new(vec![FakeRow {
        columns: columns(),
        values: oscar_row(),
    }]);
    let db = besu::Database::new(Bridge::<_, Postgres>::new(Loopback {
        driver: driver.clone(),
        source: FakeSource,
    }));

    let rows: Vec<User> = block_on(db.select(users).filter(users.id.eq(7))).unwrap();
    assert_eq!(rows, vec![oscar()]);

    let recorded = driver.recorded();
    let (sql, params) = recorded.last().expect("the driver ran the statement");
    assert!(sql.starts_with("SELECT"));
    assert_eq!(params, &[Value::Int(7)]);
}

#[test]
fn the_server_helper_reports_an_execute_count() {
    let driver = FakeDriver::new(Vec::new());
    let db = besu::Database::new(Bridge::<_, Postgres>::new(Loopback {
        driver: driver.clone(),
        source: FakeSource,
    }));

    let affected: u64 = block_on(db.insert(users).value(users.insert(
        "oscar@example.com",
        "Oscar",
        21,
    )))
    .unwrap();
    assert_eq!(affected, 1);

    let recorded = driver.recorded();
    let (sql, params) = recorded.last().expect("the driver ran the statement");
    assert!(sql.starts_with("INSERT INTO"));
    assert_eq!(params.len(), 3);
}

#[test]
fn the_bridge_dialect_spells_like_its_inner_dialect() {
    let postgres = besu::dry::<BridgeDialect<Postgres>>()
        .select(users)
        .filter(users.age.gt(18))
        .to_sql()
        .unwrap();
    assert!(postgres.sql().contains("$1"), "{}", postgres.sql());
    assert_eq!(postgres.args(), &[BridgeValue::Int(18)]);

    let mysql = besu::dry::<BridgeDialect<besu_mysql::MySql>>()
        .select(users)
        .filter(users.age.gt(18))
        .to_sql()
        .unwrap();
    assert!(mysql.sql().contains('?'), "{}", mysql.sql());
    assert!(!mysql.sql().contains('$'), "{}", mysql.sql());
}
