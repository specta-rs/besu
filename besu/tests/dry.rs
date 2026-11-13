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

use besu::types::Int8;
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
    const SUPPORTS_ARRAYS: bool = true;
    const JSON_PATH: besu_core::JsonPathStyle = besu_core::JsonPathStyle::Postgres;
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
    const JSON_PATH: besu_core::JsonPathStyle = besu_core::JsonPathStyle::Mysql;
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

/// A table that refers to itself, so a query can read it twice: every employee has at most
/// one manager, who is another employee.
#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "employees")]
pub struct Employee {
    #[besu(primary_key, auto_increment)]
    id: i64,
    name: String,
    manager_id: Option<i64>,
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

/// A scalar subquery is an expression: it renders inside parentheses wherever a value is
/// allowed, and its parameters continue the outer statement's numbering.
#[test]
fn scalar_subqueries_render_in_a_projection_and_a_comparison() {
    let db = dry::<TestDialect>();

    let sql = db
        .select(users)
        .pick((
            users.name,
            scalar(
                db.select(posts)
                    .correlate(users)
                    .filter(posts.author_id.eq(users.id))
                    .pick(count_all()),
                // The witness is a column of the intended SQL type, so no marker needs
                // importing.
                users.id,
            ),
        ))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "users"."name", (SELECT COUNT(*) FROM "posts" WHERE ("posts"."author_id" = "users"."id")) FROM "users""#
    );
    assert!(sql.args().is_empty());
    assert!(sql.error().is_none(), "{:?}", sql.error());

    let sql = db
        .select(users)
        .pick(users.id)
        .filter(users.age.gt(18))
        .filter(
            users
                .id
                .eq(scalar(db.select(posts).pick(posts.author_id), users.id)),
        )
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "users"."id" FROM "users" WHERE ("users"."age" > $1) AND ("users"."id" = (SELECT "posts"."author_id" FROM "posts"))"#
    );
    assert_eq!(sql.args(), [Value::Int(18)]);

    // The scalar subquery is rendered before the outer `WHERE`, so its parameter takes the
    // earlier placeholder even though the outer clause is written later in the builder.
    let sql = db
        .select(users)
        .pick(scalar(
            db.select(posts)
                .correlate(users)
                .filter(posts.author_id.eq(users.id))
                .filter(posts.title.eq("hello"))
                .pick(count_all()),
            users.id,
        ))
        .filter(users.age.gt(18))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT (SELECT COUNT(*) FROM "posts" WHERE ("posts"."author_id" = "users"."id") AND ("posts"."title" = $1)) FROM "users" WHERE ("users"."age" > $2)"#
    );
    assert_eq!(sql.args(), [Value::Text("hello".into()), Value::Int(18)]);
}

/// A derived table is a subquery in `FROM`: its body is written out in place, its columns
/// are addressed through the alias, and its parameters come before the outer ones.
#[test]
fn derived_tables_render_in_from() {
    let db = dry::<TestDialect>();

    let recent = derived(
        db.select(posts)
            .filter(posts.title.eq("hello"))
            .pick(posts.author_id),
        "recent",
    );
    let author = recent.column(posts.author_id);

    // The default projection reads the expression's columns through the alias.
    let sql = db.select(&recent).to_sql().unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "recent"."author_id" FROM (SELECT "posts"."author_id" FROM "posts" WHERE ("posts"."title" = $1)) AS "recent""#
    );
    assert_eq!(sql.args(), [Value::Text("hello".into())]);

    let sql = db
        .select(&recent)
        .pick((author.clone(), count_all()))
        .group_by(author)
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "recent"."author_id", COUNT(*) FROM (SELECT "posts"."author_id" FROM "posts" WHERE ("posts"."title" = $1)) AS "recent" GROUP BY "recent"."author_id""#
    );

    // A derived table is a `TableRef`, so it joins like a declared one.
    let sql = db
        .select(users)
        .inner_join(&recent, recent.column(posts.author_id).eq(users.id))
        .pick(users.name)
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "users"."name" FROM "users" INNER JOIN (SELECT "posts"."author_id" FROM "posts" WHERE ("posts"."title" = $1)) AS "recent" ON ("recent"."author_id" = "users"."id")"#
    );

    // The body is rendered inside `FROM` before the outer `WHERE`, so its parameter comes
    // first and the outer one continues the numbering.
    let author = recent.column(posts.author_id);
    let sql = db
        .select(&recent)
        .pick(author.clone())
        .filter(author.gt(0_i64))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "recent"."author_id" FROM (SELECT "posts"."author_id" FROM "posts" WHERE ("posts"."title" = $1)) AS "recent" WHERE ("recent"."author_id" > $2)"#
    );
    assert_eq!(sql.args(), [Value::Text("hello".into()), Value::Int(0)]);

    // An expression column has no portable name, so it is reached by name with a witness.
    let named = derived(
        db.select(posts).pick((posts.author_id, count_all())),
        "named",
    );
    let author = named.column(posts.author_id);
    let total = named.column_named("count", users.id);
    let sql = db
        .select(&named)
        .pick((author.clone(), total.clone()))
        .group_by(author)
        .having(total.gt(1_i64))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "named"."author_id", "named"."count" FROM (SELECT "posts"."author_id", COUNT(*) FROM "posts") AS "named" GROUP BY "named"."author_id" HAVING ("named"."count" > $1)"#
    );

    // The same statement on a dialect with backtick identifiers and `?` placeholders.
    let db = dry::<BacktickDialect>();
    let recent = derived(
        db.select(posts)
            .filter(posts.title.eq("hello"))
            .pick(posts.author_id),
        "recent",
    );
    let author = recent.column(posts.author_id);
    let sql = db
        .select(&recent)
        .pick((author.clone(), count_all()))
        .group_by(author)
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        "SELECT `recent`.`author_id`, COUNT(*) FROM (SELECT `posts`.`author_id` FROM `posts` WHERE (`posts`.`title` = ?)) AS `recent` GROUP BY `recent`.`author_id`"
    );
}

/// A derived table whose projection contains an unnamed expression cannot be read with the
/// default projection, because the three databases give that column three different names.
#[test]
fn a_derived_expression_needs_an_explicit_pick() {
    let db = dry::<TestDialect>();
    let named = derived(db.select(posts).pick(count_all()), "counted");
    let sql = db.select(&named).to_sql().unwrap();
    let error = sql.error().expect("the unnamed column must be rejected");
    assert!(error.to_string().contains("no single name"), "{error:?}");
}

/// A compound query is a statement, so every place that accepts a `SELECT` accepts it:
/// `EXISTS`, `IN`, a scalar value and a derived table. The parentheses belong to the form,
/// so the compound's branches and operators are written directly inside them.
#[test]
fn a_set_operation_is_a_subquery() {
    let db = dry::<TestDialect>();

    let sql = db
        .select(users)
        .pick(users.id)
        .filter(exists(
            db.select(users)
                .pick(users.id)
                .filter(users.age.gt(20))
                .union(db.select(users).pick(users.id).filter(users.age.lt(10))),
        ))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "users"."id" FROM "users" WHERE EXISTS (SELECT "users"."id" FROM "users" WHERE ("users"."age" > $1) UNION SELECT "users"."id" FROM "users" WHERE ("users"."age" < $2))"#
    );
    assert_eq!(sql.args(), [Value::Int(20), Value::Int(10)]);
    assert!(sql.error().is_none(), "{:?}", sql.error());

    let sql = db
        .select(users)
        .pick(users.id)
        .filter(
            users.id.in_query(
                db.select(users)
                    .pick(users.id)
                    .filter(users.age.gt(20))
                    .union(db.select(users).pick(users.id).filter(users.age.lt(10))),
            ),
        )
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "users"."id" FROM "users" WHERE ("users"."id" IN (SELECT "users"."id" FROM "users" WHERE ("users"."age" > $1) UNION SELECT "users"."id" FROM "users" WHERE ("users"."age" < $2)))"#
    );
    assert_eq!(sql.args(), [Value::Int(20), Value::Int(10)]);
    assert!(sql.error().is_none(), "{:?}", sql.error());

    // A scalar subquery needs one row and one column; the compound's own `LIMIT` applies to
    // the whole result, so it is written after the last branch and its parameter follows.
    let sql = db
        .select(users)
        .pick(scalar(
            db.select(users)
                .pick(users.id)
                .filter(users.age.gt(20))
                .union(db.select(users).pick(users.id).filter(users.age.lt(10)))
                .limit(1),
            users.id,
        ))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT (SELECT "users"."id" FROM "users" WHERE ("users"."age" > $1) UNION SELECT "users"."id" FROM "users" WHERE ("users"."age" < $2) LIMIT $3) FROM "users""#
    );
    assert_eq!(sql.args(), [Value::Int(20), Value::Int(10), Value::Int(1)]);
    assert!(sql.error().is_none(), "{:?}", sql.error());

    // A derived table reads the compound through its alias, and the default projection is the
    // compound's first branch, which is where SQL takes a compound's output names from.
    let everyone = derived(
        db.select(users)
            .pick(users.id)
            .filter(users.age.gt(20))
            .union(db.select(users).pick(users.id).filter(users.age.lt(10))),
        "everyone",
    );
    let id = everyone.column(users.id);

    let sql = db.select(&everyone).to_sql().unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "everyone"."id" FROM (SELECT "users"."id" FROM "users" WHERE ("users"."age" > $1) UNION SELECT "users"."id" FROM "users" WHERE ("users"."age" < $2)) AS "everyone""#
    );
    assert_eq!(sql.args(), [Value::Int(20), Value::Int(10)]);

    let sql = db
        .select(&everyone)
        .pick(id.clone())
        .filter(id.gt(0_i64))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "everyone"."id" FROM (SELECT "users"."id" FROM "users" WHERE ("users"."age" > $1) UNION SELECT "users"."id" FROM "users" WHERE ("users"."age" < $2)) AS "everyone" WHERE ("everyone"."id" > $3)"#
    );
    assert_eq!(sql.args(), [Value::Int(20), Value::Int(10), Value::Int(0)]);
}

/// A projected expression is named at the call site, and the handle reads its columns back by
/// that name with a type witness.
#[test]
fn a_projected_common_table_expression_is_named_at_the_call_site() {
    let db = dry::<TestDialect>();

    let totals = db
        .select(posts)
        .pick((posts.author_id, count_all()))
        .cte_named("totals", (("author_id", posts.author_id), ("total", Int8)));
    let author = totals.column_named("author_id", posts.author_id);
    let total = totals.column_named("total", Int8);

    let sql = db
        .from(&totals)
        .pick((author.clone(), total.clone()))
        .filter(total.gt(1_i64))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"WITH "totals" ("author_id", "total") AS (SELECT "posts"."author_id", COUNT(*) FROM "posts") SELECT "totals"."author_id", "totals"."total" FROM "totals" WHERE ("totals"."total" > $1)"#
    );
    assert_eq!(sql.args(), [Value::Int(1)]);
    assert!(sql.error().is_none(), "{:?}", sql.error());

    // The column list has to describe the projection. A mismatch is recorded on the statement
    // that uses the handle, because that is where the `WITH` clause is rendered.
    let bad = db
        .select(posts)
        .pick((posts.author_id, count_all()))
        .cte_named("bad", (("author_id", posts.author_id),));
    let sql = db
        .from(&bad)
        .pick(bad.column_named("author_id", posts.author_id))
        .to_sql()
        .unwrap();
    let error = sql.error().expect("the column count must be checked");
    assert!(
        error.to_string().contains("given 1 column name(s)"),
        "{error}"
    );

    // And the names have to be identifiers, because they are the names a `SELECT` reads.
    let bad_name = db
        .select(posts)
        .pick(posts.author_id)
        .cte_named("not a name", (("author_id", posts.author_id),));
    let sql = db
        .from(&bad_name)
        .pick(bad_name.column_named("author_id", posts.author_id))
        .to_sql()
        .unwrap();
    let error = sql.error().expect("the name must be an identifier");
    assert!(error.to_string().contains("plain identifier"), "{error}");

    // The same constructor exists on a compound, which is a valid `WITH` body.
    let everyone = db
        .select(users)
        .pick(users.id)
        .union(db.select(posts).pick(posts.author_id))
        .cte_named("everyone", (("id", users.id),));
    let sql = db
        .from(&everyone)
        .pick(everyone.column_named("id", users.id))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"WITH "everyone" ("id") AS (SELECT "users"."id" FROM "users" UNION SELECT "posts"."author_id" FROM "posts") SELECT "everyone"."id" FROM "everyone""#
    );
    assert!(sql.error().is_none(), "{:?}", sql.error());

    // A projected expression can be the anchor of a recursive definition, and its explicit
    // column list is written into the `WITH RECURSIVE` entry.
    let numbers = db
        .select(users)
        .pick(users.id)
        .cte_named("numbers", (("id", users.id),));
    let sql = numbers
        .clone()
        .union_all(db.select(users).pick(users.id))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"WITH RECURSIVE "numbers" ("id") AS (SELECT "users"."id" FROM "users" UNION ALL SELECT "users"."id" FROM "users") SELECT "numbers"."id" FROM "numbers""#
    );
    assert!(sql.error().is_none(), "{:?}", sql.error());
}

/// The dynamic handle takes erased clauses, so a query can be finished where the typed
/// builder's row-set parameter is in the way.
#[test]
fn a_dynamic_select_takes_erased_clauses() {
    let db = dry::<TestDialect>();

    // The expression is built while its row set is still known, then erased for the handle.
    let filter: Expr<(UserTable,), besu::types::Bool> = users.age.gte(18);
    let order: OrderClause = users.name.asc().into_order();

    let sql = db
        .select(users)
        .dynamic()
        .filter(filter.into_kind())
        .order_by(order)
        .limit(5)
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "users"."id", "users"."email", "users"."name", "users"."age", "users"."bio" FROM "users" WHERE ("users"."age" >= $1) ORDER BY "users"."name" ASC LIMIT $2"#
    );
    assert_eq!(sql.args(), [Value::Int(18), Value::Int(5)]);

    // The row type survives, so the result still decodes into the shape `pick` chose, and a
    // dynamic projection can replace the columns without changing it.
    let sql = db
        .select(users)
        .pick(users.name)
        .dynamic()
        .pick(count_all::<()>().into_kind())
        .to_sql()
        .unwrap();
    assert_eq!(sql.sql(), r#"SELECT COUNT(*) FROM "users""#);
}

/// `count` wraps the statement in a derived table, so the filters, joins, ordering and
/// pagination of the original still apply.
#[test]
fn count_wraps_a_select_and_a_compound() {
    let db = dry::<TestDialect>();

    let sql = db
        .select(users)
        .filter(users.age.gte(18))
        .limit(10)
        .count()
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT COUNT(*) FROM (SELECT "users"."id", "users"."email", "users"."name", "users"."age", "users"."bio" FROM "users" WHERE ("users"."age" >= $1) LIMIT $2) AS "besu_count""#
    );
    assert_eq!(sql.args(), [Value::Int(18), Value::Int(10)]);

    let sql = db
        .select(users)
        .pick(users.id)
        .filter(users.age.gt(20))
        .union(db.select(users).pick(users.id).filter(users.age.lt(10)))
        .limit(5)
        .count()
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT COUNT(*) FROM (SELECT "users"."id" FROM "users" WHERE ("users"."age" > $1) UNION SELECT "users"."id" FROM "users" WHERE ("users"."age" < $2) LIMIT $3) AS "besu_count""#
    );
    assert_eq!(sql.args(), [Value::Int(20), Value::Int(10), Value::Int(5)]);
}

/// An alias renames a table inside the statement, which is the only way to read one table
/// twice: every employee and the employee who manages them are rows of `employees`.
#[test]
fn an_alias_reads_a_table_under_a_second_name() {
    let db = dry::<TestDialect>();
    let manager = alias(employees, "manager");

    // `FROM "employees" AS "manager"`, with the default projection qualified by the alias.
    let sql = db.select(&manager).to_sql().unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "manager"."id", "manager"."name", "manager"."manager_id" FROM "employees" AS "manager""#
    );

    // The self-join: one table, two names, and the condition names both. The alias takes the
    // join's place in the row set, so both sides of the comparison are checked against the
    // schema and against the tables the statement really reads.
    let sql = db
        .select(employees)
        .left_join(
            &manager,
            manager.column(employees.id).eq(employees.manager_id),
        )
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "employees"."id", "employees"."name", "employees"."manager_id", "manager"."id", "manager"."name", "manager"."manager_id" FROM "employees" LEFT JOIN "employees" AS "manager" ON ("manager"."id" = "employees"."manager_id")"#
    );

    // An alias column is an expression, so it projects, filters and orders like any other.
    let manager_name = manager.column(employees.name);
    let sql = db
        .select(employees)
        .inner_join(
            &manager,
            manager.column(employees.id).eq(employees.manager_id),
        )
        .pick((employees.name, manager_name.clone()))
        .filter(manager_name.clone().like("A%"))
        .order_by(manager_name)
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "employees"."name", "manager"."name" FROM "employees" INNER JOIN "employees" AS "manager" ON ("manager"."id" = "employees"."manager_id") WHERE ("manager"."name" LIKE $1) ORDER BY "manager"."name" ASC"#
    );
    assert_eq!(sql.args(), [Value::Text("A%".into())]);

    // The same statement on a dialect with backtick identifiers and `?` placeholders.
    let db = dry::<BacktickDialect>();
    let manager = alias(employees, "manager");
    let sql = db
        .select(employees)
        .left_join(
            &manager,
            manager.column(employees.id).eq(employees.manager_id),
        )
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        "SELECT `employees`.`id`, `employees`.`name`, `employees`.`manager_id`, `manager`.`id`, `manager`.`name`, `manager`.`manager_id` FROM `employees` LEFT JOIN `employees` AS `manager` ON (`manager`.`id` = `employees`.`manager_id`)"
    );
}

/// Two aliases of one table are distinct types, so both may appear in one statement and each
/// column is checked against the alias that was actually joined. `alias` keeps its tag-0
/// meaning; `alias_as` picks another tag. Neither tag reaches the SQL.
#[test]
fn two_aliases_of_one_table_are_distinct() {
    let db = dry::<TestDialect>();
    let manager = alias(employees, "manager");
    let report = alias_as::<_, 1>(employees, "report");

    // The employee row is the `report` alias; the manager row is the same table under a
    // different tag. Both aliases are in the row set, and the condition names both.
    let sql = db
        .select(report)
        .inner_join(
            manager,
            manager
                .column(employees.id)
                .eq(report.column(employees.manager_id)),
        )
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "report"."id", "report"."name", "report"."manager_id", "manager"."id", "manager"."name", "manager"."manager_id" FROM "employees" AS "report" INNER JOIN "employees" AS "manager" ON ("manager"."id" = "report"."manager_id")"#
    );

    // A column of each alias stays distinct in the projection and the filter, even though
    // the two columns have the same base-table handle.
    let report_name = report.column(employees.name);
    let manager_name = manager.column(employees.name);
    let sql = db
        .select(report)
        .inner_join(
            &manager,
            manager
                .column(employees.id)
                .eq(report.column(employees.manager_id)),
        )
        .pick((report_name.clone(), manager_name.clone()))
        .filter(report_name.eq(manager_name))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "report"."name", "manager"."name" FROM "employees" AS "report" INNER JOIN "employees" AS "manager" ON ("manager"."id" = "report"."manager_id") WHERE ("report"."name" = "manager"."name")"#
    );

    // Retagging a handle keeps the name and makes it a third distinct alias in the row set.
    let boss = alias(employees, "boss").tag::<2>();
    let sql = db.select(&boss).to_sql().unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "boss"."id", "boss"."name", "boss"."manager_id" FROM "employees" AS "boss""#
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

#[test]
fn mysql_json_paths_are_quoted() {
    // The path is rendered as a string literal, and the closing quote is easy to drop: it
    // was missing until a documentation pass tried to execute the statement.
    let db = dry::<BacktickDialect>();
    let sql = db
        .select(events)
        .pick(besu::json_text(events.payload, "user.name"))
        .to_sql()
        .unwrap();

    assert!(
        sql.sql()
            .contains(r#"JSON_UNQUOTE(JSON_EXTRACT(`events`.`payload`, '$.user.name'))"#),
        "got: {}",
        sql.sql()
    );
}

// ---------------------------------------------------------------------------------------
// Upserts
// ---------------------------------------------------------------------------------------

/// Every upsert shape PostgreSQL and SQLite share: no target at all, a target with an index
/// predicate, and an update that reads the incoming row plus a predicate against the stored
/// one.
#[test]
fn upserts_render_the_on_conflict_shapes() {
    let db = dry::<TestDialect>();

    // No target. `ON CONFLICT DO NOTHING` is the only action that allows it.
    let sql = db
        .insert(users)
        .value(users.insert("oscar@example.com", "Oscar", 21))
        .on_conflict_all()
        .do_nothing()
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"INSERT INTO "users" ("email", "name", "age") VALUES ($1, $2, $3) ON CONFLICT DO NOTHING"#
    );
    assert_eq!(sql.args().len(), 3);
    assert!(sql.error().is_none());

    // An empty conflict target spelled as a tuple means the same thing.
    let sql = db
        .insert(users)
        .value(users.insert("oscar@example.com", "Oscar", 21))
        .on_conflict(())
        .do_nothing()
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"INSERT INTO "users" ("email", "name", "age") VALUES ($1, $2, $3) ON CONFLICT DO NOTHING"#
    );

    // A target, the index predicate, an update of the incoming row, and the update's own
    // predicate, in that order.
    let sql = db
        .insert(users)
        .value(users.insert("oscar@example.com", "Oscar", 21))
        .on_conflict(users.email)
        .on_conflict_where(users.bio.is_null())
        .do_update(users.name, excluded(users.name))
        .do_update_filter(users.age.lt(100))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"INSERT INTO "users" ("email", "name", "age") VALUES ($1, $2, $3) ON CONFLICT ("email") WHERE ("users"."bio" IS NULL) DO UPDATE SET "name" = "excluded"."name" WHERE ("users"."age" < $4)"#
    );
    assert_eq!(
        sql.args(),
        [
            Value::Text("oscar@example.com".into()),
            Value::Text("Oscar".into()),
            Value::Int(21),
            Value::Int(100),
        ]
    );
    assert!(sql.error().is_none());

    // `new_row` is `excluded` under MySQL's name, so the rendered SQL is identical.
    let sql = db
        .insert(users)
        .value(users.insert("oscar@example.com", "Oscar", 21))
        .on_conflict(users.email)
        .do_update(users.name, new_row(users.name))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"INSERT INTO "users" ("email", "name", "age") VALUES ($1, $2, $3) ON CONFLICT ("email") DO UPDATE SET "name" = "excluded"."name""#
    );

    // Each `do_update` adds a `SET` entry, and the value is type-checked against the column.
    let sql = db
        .insert(users)
        .value(users.insert("oscar@example.com", "Oscar", 21))
        .on_conflict(users.email)
        .do_update(users.name, excluded(users.name))
        .do_update(users.age, excluded(users.age))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"INSERT INTO "users" ("email", "name", "age") VALUES ($1, $2, $3) ON CONFLICT ("email") DO UPDATE SET "name" = "excluded"."name", "age" = "excluded"."age""#
    );
}

/// MySQL's upsert reads the incoming row through a row alias, and `do_nothing` needs a
/// target to assign to itself.
#[test]
fn upserts_render_the_on_duplicate_key_shapes() {
    let db = dry::<BacktickDialect>();

    let sql = db
        .insert(users)
        .value(users.insert("oscar@example.com", "Oscar", 21))
        .on_conflict(users.email)
        .do_update(users.name, new_row(users.name))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        "INSERT INTO `users` (`email`, `name`, `age`) VALUES (?, ?, ?) AS `new` ON DUPLICATE KEY UPDATE `name` = `new`.`name`"
    );
    assert!(sql.error().is_none());

    // The alias is chosen per statement, and it is stamped at render time so the call order
    // of `row_alias` and `do_update` does not matter.
    let sql = db
        .insert(users)
        .value(users.insert("oscar@example.com", "Oscar", 21))
        .on_conflict(users.email)
        .do_update(users.name, excluded(users.name))
        .row_alias("incoming")
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        "INSERT INTO `users` (`email`, `name`, `age`) VALUES (?, ?, ?) AS `incoming` ON DUPLICATE KEY UPDATE `name` = `incoming`.`name`"
    );

    // A value-only update does not mention the incoming row, so no alias is written and the
    // statement keeps working on MySQL older than 8.0.19.
    let sql = db
        .insert(users)
        .value(users.insert("oscar@example.com", "Oscar", 21))
        .on_conflict(users.email)
        .do_update(users.age, 21)
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        "INSERT INTO `users` (`email`, `name`, `age`) VALUES (?, ?, ?) ON DUPLICATE KEY UPDATE `age` = ?"
    );

    // `do_nothing` becomes a self-assignment: MySQL has no `DO NOTHING`, and `INSERT IGNORE`
    // would also swallow truncation and foreign-key errors.
    let sql = db
        .insert(users)
        .value(users.insert("oscar@example.com", "Oscar", 21))
        .on_conflict(users.email)
        .do_nothing()
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        "INSERT INTO `users` (`email`, `name`, `age`) VALUES (?, ?, ?) ON DUPLICATE KEY UPDATE `email` = `email`"
    );
    assert!(sql.error().is_none());
}

/// The forms each dialect has no grammar for are refused while building, with a message
/// that says what to do instead.
#[test]
fn an_upsert_rejects_what_the_dialect_cannot_express() {
    // PostgreSQL and SQLite require the target for `DO UPDATE`, with or without a predicate.
    let sql = dry::<TestDialect>()
        .insert(users)
        .value(users.insert("oscar@example.com", "Oscar", 21))
        .on_conflict_all()
        .do_update(users.age, 21)
        .to_sql()
        .unwrap();
    let error = sql.error().expect("`DO UPDATE` needs a target");
    assert_eq!(
        error.to_string(),
        "invalid query: `ON CONFLICT DO UPDATE` needs a conflict target on `test`; call \
         `.on_conflict(column)` to name the key it applies to, or use `.do_nothing()`"
    );

    // A predicate with no target has nowhere to attach.
    let sql = dry::<TestDialect>()
        .insert(users)
        .value(users.insert("oscar@example.com", "Oscar", 21))
        .on_conflict_all()
        .on_conflict_where(users.bio.is_null())
        .do_nothing()
        .to_sql()
        .unwrap();
    let error = sql.error().expect("an index predicate needs a target");
    assert!(
        error.to_string().contains("needs a conflict target"),
        "got: {error}"
    );

    // MySQL has neither predicate.
    let sql = dry::<BacktickDialect>()
        .insert(users)
        .value(users.insert("oscar@example.com", "Oscar", 21))
        .on_conflict(users.email)
        .on_conflict_where(users.bio.is_null())
        .do_nothing()
        .to_sql()
        .unwrap();
    let error = sql.error().expect("MySQL has no index predicate");
    assert!(matches!(error, Error::Unsupported(_)), "{error:?}");
    assert!(
        error.to_string().contains("on_conflict_where"),
        "got: {error}"
    );

    let sql = dry::<BacktickDialect>()
        .insert(users)
        .value(users.insert("oscar@example.com", "Oscar", 21))
        .on_conflict(users.email)
        .do_update(users.age, 21)
        .do_update_filter(users.age.lt(100))
        .to_sql()
        .unwrap();
    let error = sql.error().expect("MySQL has no per-row predicate");
    assert!(matches!(error, Error::Unsupported(_)), "{error:?}");
    assert!(
        error.to_string().contains("`case`"),
        "the message must point at the replacement: {error}"
    );

    // Without a target there is nothing for MySQL's `do_nothing` to assign to itself.
    let sql = dry::<BacktickDialect>()
        .insert(users)
        .value(users.insert("oscar@example.com", "Oscar", 21))
        .on_conflict_all()
        .do_nothing()
        .to_sql()
        .unwrap();
    let error = sql
        .error()
        .expect("MySQL cannot do nothing without a target");
    assert!(matches!(error, Error::Unsupported(_)), "{error:?}");
    assert!(
        error.to_string().contains("assign it to itself"),
        "the message must say what is missing: {error}"
    );
}

/// An empty `IN` list is a syntax error everywhere, so it renders as a constant predicate
/// that every planner folds away.
#[test]
fn empty_in_lists_render_a_constant_predicate() {
    let db = dry::<TestDialect>();

    let sql = db
        .select(users)
        .pick(users.id)
        .filter(users.id.in_list(Vec::<i64>::new()))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "users"."id" FROM "users" WHERE (1 = 0)"#
    );
    assert!(sql.args().is_empty());

    let sql = db
        .select(users)
        .pick(users.id)
        .filter(users.id.not_in(Vec::<i64>::new()))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "users"."id" FROM "users" WHERE (1 = 1)"#
    );

    let sql = db
        .select(users)
        .pick(users.id)
        .filter(in_list(users.name, Vec::<&str>::new()))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "users"."id" FROM "users" WHERE (1 = 0)"#
    );
}

/// `CAST` names a type with the dialect's cast spelling, which is not always the DDL one.
/// This dialect does not override it, so the default (the DDL name) must be used.
#[test]
fn a_cast_uses_the_cast_spelling_not_the_ddl_one() {
    let db = dry::<TestDialect>();
    let sql = db
        .select(users)
        .pick(raw::<(UserTable,)>("1").cast::<besu::Float8>())
        .to_sql()
        .unwrap();
    assert_eq!(sql.sql(), r#"SELECT CAST(1 AS TEXT) FROM "users""#);
    assert!(sql.error().is_none());
}

// ---------------------------------------------------------------------------------------
// Comments, operators, functions and common table expressions
// ---------------------------------------------------------------------------------------

/// A self-referencing table, so a recursive expression has something to walk.
#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "nodes")]
pub struct Node {
    #[besu(primary_key)]
    id: i64,
    parent_id: Option<i64>,
    name: String,
}

/// A comment is written after the dialect's own suffix, so it is the last thing in the
/// statement, and every builder offers one.
#[test]
fn a_statement_carries_its_comment_last() {
    let db = dry::<TestDialect>();

    let sql = db.select(users).comment("feed page").to_sql().unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "users"."id", "users"."email", "users"."name", "users"."age", "users"."bio" FROM "users" /* feed page */"#
    );

    let sql = db
        .insert(users)
        .value(users.insert("oscar@example.com", "Oscar", 21))
        .comment("signup")
        .to_sql()
        .unwrap();
    assert!(
        sql.sql().ends_with("/* signup */"),
        "comment is not last: {}",
        sql.sql()
    );

    let sql = db
        .update(users)
        .comment("backfill")
        .set(users.name, "Oscar")
        .filter(users.id.eq(1_i64))
        .to_sql()
        .unwrap();
    assert!(sql.sql().ends_with("/* backfill */"), "{}", sql.sql());

    let sql = db
        .delete(users)
        .filter(users.id.eq(1_i64))
        .comment("cleanup")
        .to_sql()
        .unwrap();
    assert!(sql.sql().ends_with("/* cleanup */"), "{}", sql.sql());
}

/// A comment containing the sequence that would end it is rewritten, so a label can never
/// turn the rest of itself into SQL.
#[test]
fn a_comment_cannot_end_itself() {
    let db = dry::<TestDialect>();
    let sql = db
        .select(users)
        .pick(users.id)
        .comment("nasty */ DROP TABLE users")
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "users"."id" FROM "users" /* nasty * / DROP TABLE users */"#
    );
}

/// `IS DISTINCT FROM` is a keyword on the standard spelling, and the parameters follow it
/// like any other comparison.
#[test]
fn null_safe_comparisons_render() {
    let db = dry::<TestDialect>();
    let sql = db
        .select(users)
        .pick(users.id)
        .filter(users.bio.is_distinct_from("x"))
        .filter(is_not_distinct_from(users.name, "Oscar"))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        r#"SELECT "users"."id" FROM "users" WHERE ("users"."bio" IS DISTINCT FROM $1) AND ("users"."name" IS NOT DISTINCT FROM $2)"#
    );
    assert!(sql.error().is_none());
}

/// A dialect that has not said how it spells a regular expression records an error rather
/// than emitting another database's operator.
#[test]
fn a_regular_expression_needs_a_dialect_capability() {
    let db = dry::<TestDialect>();
    let sql = db
        .select(users)
        .pick(users.id)
        .filter(users.name.regex_match("^O"))
        .to_sql()
        .unwrap();
    let error = sql.error().expect("a regex should be rejected");
    assert!(error.to_string().contains("regular-expression"), "{error}");
}

/// A non-recursive expression is defined where it is used, and its parameter comes before
/// the statement's own.
#[test]
fn a_common_table_expression_renders_before_the_statement() {
    let db = dry::<TestDialect>();
    let adults = db.select(users).filter(users.age.gte(18)).cte("adults");

    let sql = db
        .from(&adults)
        .filter(adults.column(users.age).lt(65))
        .to_sql()
        .unwrap();

    assert_eq!(
        sql.sql(),
        r#"WITH "adults" AS (SELECT "users"."id", "users"."email", "users"."name", "users"."age", "users"."bio" FROM "users" WHERE ("users"."age" >= $1)) SELECT "adults"."id", "adults"."email", "adults"."name", "adults"."age", "adults"."bio" FROM "adults" WHERE ("adults"."age" < $2)"#
    );
    assert_eq!(sql.args(), [Value::Int(18), Value::Int(65)]);
    assert!(sql.error().is_none());
}

/// A recursive expression is defined by the compound that extends it, and the statement
/// reads the name back out.
#[test]
fn a_recursive_common_table_expression_renders() {
    let db = dry::<TestDialect>();
    let tree = db
        .select(nodes)
        .filter(nodes.parent_id.is_null())
        .cte("tree");
    let sql = tree
        .clone()
        .union_all(
            db.select(nodes)
                .inner_join_cte(&tree, nodes.parent_id.eq(tree.column(nodes.id)))
                // A union branch has to project the same columns as the anchor, and a join
                // widens the projection to both sides, so name the side that is wanted.
                .pick(nodes),
        )
        .to_sql()
        .unwrap();

    assert!(
        sql.sql().starts_with(r#"WITH RECURSIVE "tree" AS ("#),
        "{}",
        sql.sql()
    );
    assert!(
        sql.sql()
            .contains(r#"INNER JOIN "tree" ON ("nodes"."parent_id" = "tree"."id")"#),
        "{}",
        sql.sql()
    );
    assert!(
        sql.sql()
            .ends_with(r#"SELECT "tree"."id", "tree"."parent_id", "tree"."name" FROM "tree""#),
        "{}",
        sql.sql()
    );
    assert!(sql.error().is_none());
}

/// A `FILTER` narrows the aggregate, and an `OVER` window follows it in the same expression.
#[test]
fn aggregate_filters_and_windows_render() {
    let db = dry::<TestDialect>();
    let sql = db
        .select(users)
        .pick((
            sum(users.age).filter(users.age.gt(18)),
            row_number().over(
                Window::new()
                    .partition_by(users.name)
                    .order_by(users.age.asc()),
            ),
        ))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        // `FILTER` attaches to the aggregate, so it belongs *inside* the cast. The first
        // version of this rendered `CAST(SUM(..) AS ..) FILTER (..)`, which is a syntax error
        // on every server; this assertion used to pin that broken string.
        r#"SELECT CAST((SUM("users"."age") FILTER (WHERE ("users"."age" > $1))) AS TEXT), ROW_NUMBER() OVER (PARTITION BY "users"."name" ORDER BY "users"."age" ASC) FROM "users""#
    );
    assert!(sql.error().is_none());
}

/// Two expressions in one statement are written in the order they were added, and the
/// parameters follow the text: the `WITH` clause comes first, so its parameters do too.
#[test]
fn several_common_table_expressions_render_in_order() {
    let db = dry::<TestDialect>();
    let adults = db.select(users).filter(users.age.gte(18)).cte("adults");
    let recent = db.select(posts).filter(posts.id.gt(10_i64)).cte("recent");

    let sql = db
        .select(posts)
        .inner_join_cte(&adults, adults.column(users.id).eq(posts.author_id))
        .inner_join_cte(&recent, recent.column(posts.id).eq(posts.id))
        .with_cte(&adults)
        .to_sql()
        .unwrap();

    let text = sql.sql();
    let adults_at = text.find(r#"WITH "adults" AS"#).expect("adults is defined");
    let recent_at = text.find(r#", "recent" AS"#).expect("recent is defined");
    assert!(adults_at < recent_at, "{text}");
    assert_eq!(
        sql.args(),
        [Value::Int(18), Value::Int(10)],
        "the clause comes before the statement, so its parameters do"
    );
    // Adding the same expression twice keeps one definition, because a name may only appear
    // once in a `WITH` clause.
    assert_eq!(text.matches(r#""adults" AS"#).count(), 1, "{text}");
}

/// A set query takes a comment in the same place, after the compound's own clauses.
#[test]
fn a_set_query_carries_its_comment() {
    let db = dry::<TestDialect>();
    let sql = db
        .select(users)
        .union(db.select(users))
        .comment("everyone")
        .to_sql()
        .unwrap();
    assert!(sql.sql().ends_with("/* everyone */"), "{}", sql.sql());
}

// ---------------------------------------------------------------------------------------
// Arrays and structured JSON paths
// ---------------------------------------------------------------------------------------

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "tagged")]
pub struct Tagged {
    #[besu(primary_key)]
    pub id: i64,
    pub tags: Vec<String>,
    pub payload: serde_json::Value,
}

#[test]
fn an_in_list_binds_one_array_parameter() {
    use besu::expr::in_list;

    // On PostgreSQL one array parameter replaces one parameter per value, which is what
    // keeps a large list under the 65,535-parameter limit.
    let db = dry::<TestDialect>();
    let sql = db
        .select(tagged)
        .filter(in_list(tagged.id, [1_i64, 2, 3]))
        .to_sql()
        .unwrap();
    assert!(sql.sql().contains(" = ANY($1)"), "{}", sql.sql());
    assert_eq!(sql.args().len(), 1);

    // Every other dialect has no array type, so it keeps the plain `IN (...)`.
    let db = dry::<BacktickDialect>();
    let sql = db
        .select(tagged)
        .filter(in_list(tagged.id, [1_i64, 2]))
        .to_sql()
        .unwrap();
    assert!(sql.sql().contains(" IN (?, ?)"), "{}", sql.sql());
}

#[test]
fn array_operators_are_postgres_only() {
    use besu::expr::{array_contains, array_overlaps};

    let db = dry::<TestDialect>();
    let sql = db
        .select(tagged)
        .filter(array_contains(tagged.tags, vec!["rust".to_owned()]))
        .to_sql()
        .unwrap();
    assert!(sql.sql().contains(" @> "), "{}", sql.sql());

    let sql = db
        .select(tagged)
        .filter(array_overlaps(tagged.tags, vec!["go".to_owned()]))
        .to_sql()
        .unwrap();
    assert!(sql.sql().contains(" && "), "{}", sql.sql());

    // A dialect that has not said it has arrays records an error rather than emitting the
    // operator.
    let db = dry::<BacktickDialect>();
    let sql = db
        .select(tagged)
        .filter(array_contains(tagged.tags, vec!["rust".to_owned()]))
        .to_sql()
        .unwrap();
    assert!(sql.error().is_some());
}

#[test]
fn a_json_path_escapes_each_dialect_correctly() {
    use besu::expr::{JsonPath, json_at_text};

    let path = JsonPath::root().key("a.b").key("c,d").index(0);

    // PostgreSQL takes a `text[]`, so a key with a dot is written as one element and an
    // element with a comma is quoted.
    let db = dry::<TestDialect>();
    let sql = db
        .select(tagged)
        .pick(json_at_text(tagged.payload, path.clone()))
        .to_sql()
        .unwrap();
    assert!(sql.sql().contains("'{a.b,\"c,d\",0}'"), "{}", sql.sql());

    // MySQL takes a JSON path, where a member with a dot is quoted with `"`.
    let db = dry::<BacktickDialect>();
    let sql = db
        .select(tagged)
        .pick(json_at_text(tagged.payload, path))
        .to_sql()
        .unwrap();
    assert!(
        sql.sql()
            .contains(r#"JSON_EXTRACT(`tagged`.`payload`, '$."a.b"."c,d"[0]')"#),
        "{}",
        sql.sql()
    );
}

#[test]
fn inspect_leaves_a_question_mark_operator_alone() {
    // PostgreSQL spells parameters `$n`, so a bare `?` in the statement is SQL — the `jsonb ?`
    // operator. `inspect` substituted every `?` with the next parameter, which both corrupted
    // the text and shifted every later parameter.
    let mut sql = besu_core::Sql::<TestDialect>::raw(r#"SELECT $1::jsonb ? 'a' AND $2 = 1"#);
    sql.push_arg(besu::Value::Json(br#"{"a":1}"#.to_vec()));
    sql.push_arg(besu::Value::Int(1));

    let text = alloc_to_string(&sql.inspect());
    assert!(text.contains("? 'a'"), "got: {text}");
    assert!(text.contains("AND 1 = 1"), "got: {text}");
}

fn alloc_to_string(display: &impl core::fmt::Display) -> String {
    use core::fmt::Write as _;
    let mut out = String::new();
    let _ = write!(out, "{display}");
    out
}
