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

use besu::{Dialect, Driver, Error, TypeDef, Value, dry, prelude::*};
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

/// A second dialect, so the assertions prove the SQL is dialect-driven rather than
/// hard-coded: backtick identifiers and `?` placeholders, the way MySQL and SQLite spell
/// them, and no `ALL` form of `INTERSECT` or `EXCEPT`, the way SQLite does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BacktickDialect;

impl Dialect for BacktickDialect {
    type Value = Value;

    const NAME: &'static str = "backtick";
    const PLACEHOLDER: Placeholder = Placeholder::Question;
    const SUPPORTS_RETURNING: bool = false;
    const SUPPORTS_ILIKE: bool = false;
    const SUPPORTS_FULL_JOIN: bool = false;
    const SUPPORTS_INTERSECT_ALL: bool = false;
    const SUPPORTS_EXCEPT_ALL: bool = false;
    const UPSERT: UpsertStyle = UpsertStyle::OnDuplicateKey;

    fn bind(value: Value) -> Result<Self::Value, Error> {
        Ok(value)
    }

    fn write_placeholder(out: &mut String, _index: usize) {
        out.push('?');
    }

    fn write_ident(out: &mut String, ident: &str) {
        out.push('`');
        for ch in ident.chars() {
            if ch == '`' {
                out.push('`');
            }
            out.push(ch);
        }
        out.push('`');
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

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "posts")]
pub struct Post {
    #[besu(primary_key, auto_increment)]
    id: i64,
    title: String,
    #[besu(references = "users.id")]
    author_id: i64,
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

/// A compound query is one statement: the branches render in order, then the compound's own
/// `ORDER BY` and `LIMIT`. The parameters interleave the same way, which is what proves the
/// branch bindings are not separated from their placeholders.
#[test]
fn set_operations_render_the_compound_the_server_would_run() {
    let db = dry::<TestDialect>();

    let sql = db
        .select(users)
        .filter(users.age.gt(20))
        .union(db.select(users).filter(users.age.lt(10)))
        .order_by(users.name.asc())
        .limit(5)
        .to_sql()
        .unwrap();

    assert_eq!(
        sql.sql(),
        r#"SELECT "users"."id", "users"."email", "users"."name", "users"."age", "users"."bio" FROM "users" WHERE ("users"."age" > $1) UNION SELECT "users"."id", "users"."email", "users"."name", "users"."age", "users"."bio" FROM "users" WHERE ("users"."age" < $2) ORDER BY "name" ASC LIMIT $3"#
    );
    assert_eq!(sql.args(), [Value::Int(20), Value::Int(10), Value::Int(5)]);
    assert!(sql.error().is_none());
}

/// Every operator, spelled exactly once, and a chain that proves the operators accumulate
/// rather than replacing each other.
#[test]
fn every_set_operator_renders() {
    let db = dry::<TestDialect>();

    let union = db
        .select(users)
        .pick(users.id)
        .union(db.select(users).pick(users.id));
    assert_eq!(
        union.to_sql().unwrap().sql(),
        r#"SELECT "users"."id" FROM "users" UNION SELECT "users"."id" FROM "users""#
    );

    let union_all = db
        .select(users)
        .pick(users.id)
        .union_all(db.select(users).pick(users.id));
    assert_eq!(
        union_all.to_sql().unwrap().sql(),
        r#"SELECT "users"."id" FROM "users" UNION ALL SELECT "users"."id" FROM "users""#
    );

    let intersect = db
        .select(users)
        .pick(users.id)
        .intersect(db.select(users).pick(users.id));
    assert_eq!(
        intersect.to_sql().unwrap().sql(),
        r#"SELECT "users"."id" FROM "users" INTERSECT SELECT "users"."id" FROM "users""#
    );

    let intersect_all = db
        .select(users)
        .pick(users.id)
        .intersect_all(db.select(users).pick(users.id));
    assert_eq!(
        intersect_all.to_sql().unwrap().sql(),
        r#"SELECT "users"."id" FROM "users" INTERSECT ALL SELECT "users"."id" FROM "users""#
    );

    let except = db
        .select(users)
        .pick(users.id)
        .except(db.select(users).pick(users.id));
    assert_eq!(
        except.to_sql().unwrap().sql(),
        r#"SELECT "users"."id" FROM "users" EXCEPT SELECT "users"."id" FROM "users""#
    );

    let except_all = db
        .select(users)
        .pick(users.id)
        .except_all(db.select(users).pick(users.id));
    assert_eq!(
        except_all.to_sql().unwrap().sql(),
        r#"SELECT "users"."id" FROM "users" EXCEPT ALL SELECT "users"."id" FROM "users""#
    );

    // A chain keeps every operator, in order, and every branch's parameters in order.
    let chained = db
        .select(users)
        .pick(users.id)
        .filter(users.age.gt(1))
        .union(db.select(users).pick(users.id).filter(users.age.gt(2)))
        .except(db.select(users).pick(users.id).filter(users.age.gt(3)));
    let sql = chained.to_sql().unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "users"."id" FROM "users" WHERE ("users"."age" > $1) UNION SELECT "users"."id" FROM "users" WHERE ("users"."age" > $2) EXCEPT SELECT "users"."id" FROM "users" WHERE ("users"."age" > $3)"#
    );
    assert_eq!(sql.args(), [Value::Int(1), Value::Int(2), Value::Int(3)]);
}

/// The same query on a dialect with different identifiers and placeholders, so the
/// assertions cannot be satisfied by a string that happens to be hard-coded.
#[test]
fn set_operations_are_spelled_by_the_dialect() {
    let db = dry::<BacktickDialect>();

    let sql = db
        .select(users)
        .pick(users.id)
        .filter(users.age.gt(20))
        .union(db.select(users).pick(users.id).filter(users.age.lt(10)))
        .order_by(users.name.asc())
        .offset(4)
        .to_sql()
        .unwrap();

    assert_eq!(
        sql.sql(),
        "SELECT `users`.`id` FROM `users` WHERE (`users`.`age` > ?) UNION SELECT `users`.`id` FROM `users` WHERE (`users`.`age` < ?) ORDER BY `name` ASC OFFSET ?"
    );
    assert_eq!(sql.args(), [Value::Int(20), Value::Int(10), Value::Int(4)]);
}

/// A branch's own `ORDER BY`, `LIMIT` or `OFFSET` belongs to the branch in SQL, but MySQL
/// only accepts it inside a derived table. Dropping it silently would run a different query,
/// so it is an error that names the branch.
#[test]
fn a_set_branch_cannot_carry_its_own_pagination() {
    let db = dry::<TestDialect>();

    let sql = db
        .select(users)
        .pick(users.id)
        .union(db.select(users).pick(users.id).limit(3))
        .to_sql()
        .unwrap();

    let error = sql.error().expect("the branch limit must be rejected");
    assert!(matches!(error, Error::Unsupported(_)), "{error:?}");
    assert_eq!(
        error.to_string(),
        "unsupported: `LIMIT` on branch 2 of a set operation is not portable; call `limit` \
         on the set query so it applies to the whole result"
    );
    assert!(
        error.to_string().contains("branch 2"),
        "the message must name the branch: {error}"
    );

    let sql = db
        .select(users)
        .pick(users.id)
        .order_by(users.id.asc())
        .union(db.select(users).pick(users.id))
        .to_sql()
        .unwrap();
    let error = sql.error().expect("the branch order must be rejected");
    assert!(error.to_string().contains("branch 1"), "{error}");
}

/// `INTERSECT ALL` and `EXCEPT ALL` are not SQLite syntax; the other two operators are.
#[test]
fn the_all_operators_are_rejected_where_the_dialect_has_no_such_spelling() {
    let db = dry::<BacktickDialect>();

    let sql = db
        .select(users)
        .pick(users.id)
        .intersect_all(db.select(users).pick(users.id))
        .to_sql()
        .unwrap();
    let error = sql.error().expect("intersect all must be rejected");
    assert_eq!(
        error.to_string(),
        "unsupported: `backtick` has no `INTERSECT ALL`; use `intersect`, which removes \
         duplicates instead"
    );

    let sql = db
        .select(users)
        .pick(users.id)
        .except_all(db.select(users).pick(users.id))
        .to_sql()
        .unwrap();
    let error = sql.error().expect("except all must be rejected");
    assert_eq!(
        error.to_string(),
        "unsupported: `backtick` has no `EXCEPT ALL`; use `except`, which removes duplicates \
         instead"
    );

    // The dialects that do spell them are left alone.
    let sql = dry::<TestDialect>()
        .select(users)
        .pick(users.id)
        .except_all(dry::<TestDialect>().select(users).pick(users.id))
        .to_sql()
        .unwrap();
    assert!(sql.error().is_none(), "{:?}", sql.error());
}

/// `exists` renders the subquery into the same statement, so its parameters take the next
/// placeholder rather than starting again at one.
#[test]
fn exists_renders_a_correlated_subquery() {
    let db = dry::<TestDialect>();

    let sql = db
        .select(users)
        .filter(users.age.gt(18))
        .filter(exists(
            db.select(posts)
                .correlate(users)
                .filter(posts.author_id.eq(users.id))
                .filter(posts.title.eq("hello")),
        ))
        .to_sql()
        .unwrap();

    assert_eq!(
        sql.sql(),
        r#"SELECT "users"."id", "users"."email", "users"."name", "users"."age", "users"."bio" FROM "users" WHERE ("users"."age" > $1) AND EXISTS (SELECT "posts"."id", "posts"."title", "posts"."author_id" FROM "posts" WHERE ("posts"."author_id" = "users"."id") AND ("posts"."title" = $2))"#
    );
    assert_eq!(sql.args(), [Value::Int(18), Value::Text("hello".into())]);
    assert!(sql.error().is_none());
}

#[test]
fn not_exists_and_in_queries_render() {
    let db = dry::<TestDialect>();

    let sql = db
        .select(users)
        .pick(users.id)
        .filter(not_exists(
            db.select(posts)
                .correlate(users)
                .filter(posts.author_id.eq(users.id)),
        ))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "users"."id" FROM "users" WHERE NOT EXISTS (SELECT "posts"."id", "posts"."title", "posts"."author_id" FROM "posts" WHERE ("posts"."author_id" = "users"."id"))"#
    );

    let sql = db
        .select(users)
        .pick(users.id)
        .filter(users.id.in_query(db.select(posts).pick(posts.author_id)))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "users"."id" FROM "users" WHERE ("users"."id" IN (SELECT "posts"."author_id" FROM "posts"))"#
    );

    let sql = db
        .select(users)
        .pick(users.id)
        .filter(
            users
                .id
                .not_in_query(db.select(posts).pick(posts.author_id)),
        )
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "users"."id" FROM "users" WHERE ("users"."id" NOT IN (SELECT "posts"."author_id" FROM "posts"))"#
    );

    // The same queries on the other dialect.
    let db = dry::<BacktickDialect>();
    let sql = db
        .select(users)
        .pick(users.id)
        .filter(users.id.in_query(db.select(posts).pick(posts.author_id)))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        "SELECT `users`.`id` FROM `users` WHERE (`users`.`id` IN (SELECT `posts`.`author_id` FROM `posts`))"
    );
}

/// A subquery that produces more than one column cannot be used with `IN`. The type system
/// rules this out for every accepted call, so the check is a backstop and is asserted
/// through the rendering of a valid one alongside it.
#[test]
fn in_query_takes_exactly_one_column() {
    let db = dry::<TestDialect>();

    let sql = db
        .select(users)
        .pick(users.id)
        .filter(users.id.in_query(db.select(posts).pick(posts.author_id)))
        .to_sql()
        .unwrap();
    assert!(sql.error().is_none(), "{:?}", sql.error());

    // An `EXISTS` subquery may select anything, because the projection is ignored.
    let sql = db
        .select(users)
        .filter(exists(db.select(posts)))
        .to_sql()
        .unwrap();
    assert!(sql.error().is_none(), "{:?}", sql.error());
    assert_eq!(
        sql.sql(),
        r#"SELECT "users"."id", "users"."email", "users"."name", "users"."age", "users"."bio" FROM "users" WHERE EXISTS (SELECT "posts"."id", "posts"."title", "posts"."author_id" FROM "posts")"#
    );
}

#[test]
fn a_built_statement_can_be_wrapped_in_raw_sql() {
    // `EXPLAIN`, `INSERT ... SELECT` and `CREATE TABLE ... AS` have no builder, but they do
    // take a query. Wrapping one keeps its placeholders and its parameters in order.
    let db = dry::<TestDialect>();
    let sql = db.select(users).filter(users.age.gt(18)).to_sql().unwrap();

    let wrapped = besu::Fragment::from(sql).wrap("EXPLAIN ", "");
    assert!(
        wrapped.sql().starts_with("EXPLAIN SELECT "),
        "{}",
        wrapped.sql()
    );
    assert!(wrapped.sql().contains(r#"("users"."age" > $1)"#));

    let rendered = wrapped.render::<TestDialect>();
    assert!(rendered.error().is_none());
    assert_eq!(rendered.args().len(), 1);
    assert!(rendered.sql().starts_with("EXPLAIN SELECT "));
}

#[test]
fn a_rendered_statement_refuses_a_different_dialect() {
    // The placeholders are already `$1`, so handing the text to a `?` dialect would produce
    // a statement that parses and binds nothing.
    let db = dry::<TestDialect>();
    let sql = db.select(users).filter(users.age.gt(18)).to_sql().unwrap();
    let wrapped = besu::Fragment::from(sql);

    let rendered = wrapped.render::<BacktickDialect>();
    let error = rendered
        .error()
        .expect("a dialect mismatch must be an error");
    assert!(
        error.to_string().contains("rendered for `test`"),
        "got: {error}"
    );
}

#[test]
fn mixing_raw_sql_with_a_rendered_statement_is_refused() {
    let db = dry::<TestDialect>();
    let sql = db.select(users).filter(users.age.gt(18)).to_sql().unwrap();

    let mut fragment = besu::Fragment::from(sql);
    fragment.push(besu::Fragment::new(" FOR UPDATE"));

    let rendered = fragment.render::<TestDialect>();
    assert!(
        rendered.error().is_some(),
        "mixing numbered and unnumbered placeholders must be refused"
    );
}

/// `INTERSECT` binds tighter than `UNION` and `EXCEPT` on PostgreSQL and MySQL, and SQLite
/// has no precedence at all: it evaluates every compound operator left to right. The two
/// cannot be mixed in one unparenthesised chain without the statement meaning different
/// things on different databases, so it is refused rather than guessed at.
#[test]
fn mixing_intersect_with_union_is_rejected() {
    let db = dry::<TestDialect>();

    let sql = db
        .select(users)
        .pick(users.id)
        .union(db.select(users).pick(users.id))
        .intersect(db.select(users).pick(users.id))
        .to_sql()
        .unwrap();
    let error = sql.error().expect("a mixed chain must be rejected");
    assert_eq!(
        error.to_string(),
        "unsupported: this set operation mixes `INTERSECT` with `UNION` or `EXCEPT`, which \
         PostgreSQL and MySQL group differently from SQLite; run one operator per set query, \
         or combine them through a subquery"
    );

    // Families that do agree with each other stay legal, and they are the ones whose
    // left-to-right reading every dialect shares.
    let sql = db
        .select(users)
        .pick(users.id)
        .union(db.select(users).pick(users.id))
        .except(db.select(users).pick(users.id))
        .to_sql()
        .unwrap();
    assert!(sql.error().is_none(), "{:?}", sql.error());

    let sql = db
        .select(users)
        .pick(users.id)
        .intersect(db.select(users).pick(users.id))
        .intersect_all(db.select(users).pick(users.id))
        .to_sql()
        .unwrap();
    assert!(sql.error().is_none(), "{:?}", sql.error());
}
