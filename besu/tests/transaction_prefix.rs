//! A transaction is an executor, so a query built inside one has to name the same physical
//! tables the database does. The prefix lives on the database, and used to be dropped when
//! `begin` handed back a transaction.
#![cfg(feature = "derive")]
#![allow(missing_docs, clippy::unwrap_used)]

use std::{
    borrow::Cow,
    future::Future,
    marker::PhantomData,
    pin::pin,
    task::{Context, Poll, Waker},
};

use besu::prelude::*;
use besu::{Database, Dialect, Driver, Error, RowStream, TransactionDriver, TypeDef, Value};
use besu_core::{Placeholder, UpsertStyle};

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

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "users")]
pub struct User {
    #[besu(primary_key)]
    id: i64,
}

#[derive(Clone, Default)]
struct Fake;

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

    async fn query(&self, _sql: &besu::Sql<Self::Dialect>) -> Result<Vec<Self::Row>, Self::Error> {
        Ok(Vec::new())
    }

    async fn execute(&self, _sql: &besu::Sql<Self::Dialect>) -> Result<Self::Output, Self::Error> {
        Ok(0)
    }
}

struct FakeTransaction<'a>(PhantomData<&'a ()>);

impl TransactionDriver for Fake {
    type Transaction<'a> = FakeTransaction<'a>;

    async fn begin<'a>(&'a self) -> Result<Self::Transaction<'a>, Self::Error> {
        Ok(FakeTransaction(PhantomData))
    }

    async fn commit<'a>(_tx: Self::Transaction<'a>) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn rollback<'a>(_tx: Self::Transaction<'a>) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn txn_query<'c, 'a>(
        _tx: &'c mut Self::Transaction<'a>,
        _sql: &besu::Sql<Self::Dialect>,
    ) -> Result<Vec<Self::Row>, Self::Error> {
        Ok(Vec::new())
    }

    async fn txn_execute<'c, 'a>(
        _tx: &'c mut Self::Transaction<'a>,
        _sql: &besu::Sql<Self::Dialect>,
    ) -> Result<Self::Output, Self::Error> {
        Ok(0)
    }

    async fn txn_query_stream<'c, 'a>(
        _tx: &'c mut Self::Transaction<'a>,
        _sql: &'c besu::Sql<Self::Dialect>,
    ) -> Result<RowStream<'c, Self>, Self::Error> {
        let rows: Vec<Result<i64, Error>> = Vec::new();
        Ok(Box::pin(besu::futures_stream::iter(rows)))
    }
}

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

#[test]
fn a_transaction_keeps_the_database_table_prefix() {
    let db = Database::prefixed(Fake, "tenant_42_");
    let mut tx = block_on(db.begin()).expect("the fake driver begins");

    let sql = tx.select(users).to_sql().unwrap();
    assert!(
        sql.sql().contains(r#"FROM "tenant_42_users""#),
        "the FROM clause lost the prefix: {}",
        sql.sql()
    );
    assert!(
        sql.sql().contains(r#""tenant_42_users"."id""#),
        "a qualified column lost the prefix: {}",
        sql.sql()
    );

    let sql = tx.insert(users).value(users.insert(1)).to_sql().unwrap();
    assert!(
        sql.sql().contains(r#"INSERT INTO "tenant_42_users""#),
        "the insert lost the prefix: {}",
        sql.sql()
    );
}
