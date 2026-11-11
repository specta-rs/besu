//! SQL generation with no database, no socket and no clock.
//!
//! `besu::dry::<D>()` is the whole browser story: a web front end builds a typed query and
//! renders the exact SQL a server will run, without a driver, an async runtime or a
//! filesystem anywhere in the path. These tests are plain `#[test]`s, so they run on every
//! platform and keep the `wasm32-unknown-unknown` build honest without a browser or a wasm
//! runtime. See `docs/architecture.md`.
//!
//! The dialect is written out here rather than taken from `besu-sqlite`: a driver crate
//! depends on `besu`, so a test in this crate cannot use one. Everything below is the
//! public API, which is the point.
#![cfg(feature = "derive")]
#![allow(missing_docs, clippy::panic, clippy::unwrap_used)]

use std::{
    borrow::Cow,
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};

use besu::{
    Dialect, Driver, Error, TypeDef, Value, dry,
    prelude::{Table, now},
};
use besu_core::{Placeholder, UpsertStyle};

/// A stand-in dialect: `$n` placeholders and double-quoted identifiers, which is enough to
/// make the rendering path visible in the assertions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TestDialect;

impl Dialect for TestDialect {
    type Value = Value;

    const NAME: &'static str = "test";
    const PLACEHOLDER: Placeholder = Placeholder::Dollar;
    const SUPPORTS_RETURNING: bool = true;
    const SUPPORTS_ILIKE: bool = false;
    const SUPPORTS_FULL_JOIN: bool = true;
    const UPSERT: UpsertStyle = UpsertStyle::OnConflict;

    fn bind(value: Value) -> Result<Self::Value, Error> {
        Ok(value)
    }

    fn write_placeholder(out: &mut String, index: usize) {
        out.push('$');
        out.push_str(&(index + 1).to_string());
    }

    fn type_name(_ty: TypeDef) -> Cow<'static, str> {
        Cow::Borrowed("TEXT")
    }
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "users")]
pub struct User {
    #[besu(primary_key, auto_increment)]
    id: i64,
    #[besu(unique)]
    email: String,
    name: String,
    age: i32,
    bio: Option<String>,
}

/// A table whose column types come from `chrono`, `uuid` and `serde_json`, the three
/// dependencies the wasm work had to verify on the target.
#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "events")]
pub struct Event {
    #[besu(primary_key)]
    id: uuid::Uuid,
    at: chrono::DateTime<chrono::Utc>,
    payload: serde_json::Value,
}

#[test]
fn select_renders_the_same_sql_a_server_would_run() {
    let db = dry::<TestDialect>();
    let sql = db
        .select(users)
        .filter(users.age.gt(18))
        .order_by(users.email.asc())
        .limit(50)
        .to_sql()
        .unwrap();

    assert_eq!(
        sql.sql(),
        r#"SELECT "users"."id", "users"."email", "users"."name", "users"."age", "users"."bio" FROM "users" WHERE ("users"."age" > $1) ORDER BY "users"."email" ASC LIMIT $2"#
    );
    assert_eq!(sql.args(), [Value::Int(18), Value::Int(50)]);
    assert!(sql.error().is_none());
}

#[test]
fn every_statement_kind_renders() {
    let db = dry::<TestDialect>();

    let insert = db
        .insert(users)
        .value(users.insert("oscar@example.com", "Oscar", 21))
        .to_sql()
        .unwrap();
    assert_eq!(
        insert.sql(),
        r#"INSERT INTO "users" ("email", "name", "age") VALUES ($1, $2, $3)"#
    );
    assert_eq!(
        insert.args(),
        [
            Value::Text("oscar@example.com".into()),
            Value::Text("Oscar".into()),
            Value::Int(21),
        ]
    );

    let update = db
        .update(users)
        .set(users.name, "Oscar")
        .filter(users.id.eq(1_i64))
        .to_sql()
        .unwrap();
    assert_eq!(
        update.sql(),
        r#"UPDATE "users" SET "name" = $1 WHERE ("users"."id" = $2)"#
    );

    let delete = db
        .delete(users)
        .filter(users.id.eq(1_i64))
        .to_sql()
        .unwrap();
    assert_eq!(
        delete.sql(),
        r#"DELETE FROM "users" WHERE ("users"."id" = $1)"#
    );
}

/// `chrono`, `uuid` and `serde_json` are on the wasm path, so they have to be ordinary
/// values rather than services: nothing reads a clock and nothing touches the operating
/// system. `now()` here is the SQL function, rendered as text for the database to evaluate.
#[test]
fn timestamps_uuids_and_json_are_just_values() {
    let db = dry::<TestDialect>();
    let id = uuid::Uuid::from_u128(1);
    let at = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let payload = serde_json::json!({ "kind": "signup" });

    let insert = db
        .insert(events)
        .value(events.insert(id, at, payload.clone()))
        .to_sql()
        .unwrap();

    assert_eq!(
        insert.sql(),
        r#"INSERT INTO "events" ("id", "at", "payload") VALUES ($1, $2, $3)"#
    );
    assert_eq!(
        insert.args(),
        [
            Value::Uuid(id),
            Value::TimestampTz(at),
            Value::from(payload)
        ]
    );

    let select = db
        .select(events)
        .filter(events.at.lt(now()))
        .to_sql()
        .unwrap();
    assert_eq!(
        select.sql(),
        r#"SELECT "events"."id", "events"."at", "events"."payload" FROM "events" WHERE ("events"."at" < CURRENT_TIMESTAMP)"#
    );
    assert!(select.args().is_empty());
}

/// There is no database to send a statement to, so executing one is an error rather than a
/// panic. That is what a browser gets if it tries to run a query instead of rendering it.
///
/// The query is handed straight to the driver, because decoding a row is each driver's job
/// and a dry database has no rows to decode.
#[test]
fn a_dry_database_refuses_to_execute_instead_of_panicking() {
    let db = dry::<TestDialect>();
    let sql = db.select(users).filter(users.age.gt(18)).to_sql().unwrap();
    let mut query = pin!(db.driver().query(&sql));

    let error = match query.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(Err(error)) => error,
        Poll::Ready(Ok(_)) => panic!("a dry executor has no rows to hand back"),
        Poll::Pending => panic!("a dry executor resolves immediately"),
    };

    assert!(matches!(error, Error::Unsupported(_)), "{error:?}");
    assert!(
        error.to_string().contains("no database attached"),
        "{error}"
    );
}
