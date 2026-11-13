# Connect to MySQL

Besu ships two drivers for MySQL:

- `besu_mysql::Sqlx` wraps a sqlx `MySqlPool`. It is the default and is what `Database` names.
- `besu_mysql::Native` wraps a `mysql_async::Pool`, behind the additive `native` feature. It
  has no sqlx in its path and is the driver to reach for when the dependency graph matters.
  With both features enabled, `Database` is still the sqlx driver; `NativeDatabase` names the
  native one.

Everything below applies to both unless a section says otherwise. The only interesting
decision for `Sqlx` is whether to let the driver pin the session to UTF-8 and UTC, which is
what `new` does and `connect` does not; `Native` makes the same choice.

## A connection string

```rust
use besu_mysql::Sqlx;

let db = besu::Database::new(
    Sqlx::connect("mysql://root:besu@localhost:3306/app").await?,
);
```

`Sqlx::connect` parses the URL and changes nothing else, so a charset or timezone parameter in
the URL is the one that applies.

## The recommended constructor

```rust
let db = besu::Database::new(
    Sqlx::new("mysql://root:besu@localhost:3306/app").await?,
);
```

`Sqlx::new` parses the same URL and then sets `charset=utf8mb4` and `timezone=+00:00`. The
first matters because `utf8mb4` is the only character set that stores every Unicode character,
including emoji; the second because Besu encodes `chrono` timestamps as UTC, and a session in
another timezone would silently shift a `TIMESTAMP` column on the way in or out.

## Your own pool

`from_pool` is the way to change the connection limit or the timeouts. This needs `sqlx` as a
direct dependency:

```toml
[dependencies]
sqlx = { version = "0.8", default-features = false, features = ["mysql", "runtime-tokio"] }
```

```rust
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions};

let pool = MySqlPoolOptions::new()
    .max_connections(20)
    .acquire_timeout(std::time::Duration::from_secs(5))
    .connect_with(
        "mysql://root:besu@localhost:3306/app"
            .parse::<MySqlConnectOptions>()?
            .charset("utf8mb4")
            .timezone("+00:00".to_owned()),
    )
    .await?;

let db = besu::Database::new(besu_mysql::Sqlx::from_pool(pool));
```

`Sqlx::new` does exactly this, minus the connection limit and the timeout.

### Two options worth knowing

`test_before_acquire` defaults to `true`, which sends a `COM_PING` round trip on every
acquire. That is by far the largest per-call cost in this driver: switching it off measured
18% off a 1,000-row select and 15% off a single insert. It is left on because a connection
dropped while idle then surfaces as a transparent reconnect instead of a failed query — but if
you would rather have the 18%, `from_pool` is the way:

```rust
let pool = MySqlPoolOptions::new()
    .test_before_acquire(false)
    .connect(url)
    .await?;
```

`statement_cache_capacity` is sqlx's default of 100 prepared statements per connection. Besu
builds a distinct SQL string per query shape, so an application with many shapes will evict;
raise it on the pool if that shows up in `SHOW GLOBAL STATUS LIKE 'Prepared_stmt%'`.

## The pool underneath

```rust
let pool = db.driver().pool();
```

On `Sqlx`, `pool()` returns the `MySqlPool` for anything Besu does not expose, such as closing
it at shutdown. On `Native` it returns the `mysql_async::Pool`. `db.driver()` is the driver
itself.

## Transactions

```rust
let mut tx = db.begin().await?;
tx.insert(users).value(users.insert("a@example.com", "A", 21)).await?;
tx.commit().await?;
```

`begin` borrows the database and keeps every statement on one connection; `commit` and
`rollback` consume the handle. `db.transaction(..)` commits on `Ok` and rolls back on `Err`,
and `tx.savepoint("name", ..)` nests. Both drivers behave the same here; a native transaction
owns one pooled connection, and dropping it without committing marks it for rollback so the
pool cleans it up before reuse.

## The native driver

Enable the additive feature (sqlx stays available and stays the default):

```toml
[dependencies]
besu-mysql = { version = "0.1", features = ["native"] }
```

```rust
use besu_mysql::Native;

let db = besu::Database::new(
    Native::new("mysql://root:besu@localhost:3306/app").await?,
);
```

`Native::new` parses the URL and pins the session with `SET NAMES utf8mb4` and
`SET time_zone = '+00:00'`. `Native::connect` uses the URL's connection options exactly as
written. Both cap the pool at ten connections (the sqlx driver's maximum) and open them
lazily. `Native::from_pool` takes a `mysql_async::Pool` you built yourself, and `pool()` hands
the one it owns back for anything Besu does not expose.

The pool holds at most ten connections, opened lazily. `mysql_async` caches prepared
statements per connection, keyed by SQL text, so a repeated query shape skips the prepare
round trip. A connection is not reset when it goes back to the pool (the same choice the sqlx
driver makes), so that cache and the pinned session survive; the pool still rolls back an open
transaction and drains an unread result before reusing a connection. Build a pool with
`reset_connection` enabled and pass it to `Native::from_pool` if you want every checkout to
start from a clean session, at the cost of a round trip and the statement cache.

### TLS

The native driver is compiled without a `mysql_async` TLS feature, so connections are
unencrypted. `mysql_async` offers TLS as the `rustls-tls` (with a provider such as `ring`) or
`native-tls-tls` feature plus `SslOpts` on `Opts`; Besu does not yet expose a constructor that
accepts those, so enabling TLS is an API change rather than a dependency flip. A URL that asks
for TLS (`require_ssl=true`) hits `mysql_async`'s own guard, which panics with a message
naming the features to enable; it never silently goes unencrypted.

If you need TLS today, enable the `mysql_async` feature yourself and pass a pool built with
`SslOpts` to `Native::from_pool`. Cargo unifies the features of a single `mysql_async`
version, so the driver's dependency gets the same TLS backend:

```toml
[dependencies]
mysql_async = { version = "0.36", features = ["default-rustls-ring"] }
besu-mysql = { version = "0.1", features = ["native"] }
```

TLS is never required by Besu; it is only ever the caller's choice.

### Writes

The native driver's output carries both numbers MySQL reports:

```rust
let output = db.insert(users).value(users.insert("a@example.com", "A", 21)).await?;
let id: Option<u64> = output.last_insert_id();
assert_eq!(output.rows_affected(), 1);
```

### Streaming

`db.stream(..)` is a real stream on both drivers. The native driver checks a pooled
connection out, forwards rows from the server over a bounded channel, and returns the
connection when the stream is dropped; a slow consumer applies backpressure. Inside a
transaction the stream borrows the transaction's connection and needs no task.

## No connection at all

```rust
let db = besu::dry::<besu_mysql::MySql>();

let sql = db.select(users).filter(users.age.gte(18)).to_sql()?;
assert!(sql.sql().contains("WHERE (`users`.`age` >= ?)"));
```

`besu::dry` gives the full builder and an exact `to_sql()` with no server. Executing one
returns an error that points at `to_sql`.

## Gotchas

- MySQL has no `RETURNING`. `Insert::returning` compiles and then fails when the SQL is built,
  with "`mysql` does not support `RETURNING`". Read the row back by a unique column, or use
  `last_insert_id()` from the insert's output (`MySqlQueryResult` on sqlx, `NativeOutput` on
  the native driver).
- A statement that is not pinned to `utf8mb4` can mangle text and reject an emoji. Use
  `Sqlx::new`/`Native::new`, or set the charset on a pool you build yourself. The same goes
  for the `+00:00` session timezone.
- A session timezone is not the same thing as a timezone-aware column. MySQL's
  `DATETIME(6)` stores no offset; Besu sends and reads UTC and relies on the pinned session.
- DDL causes an implicit commit on MySQL, so wrapping a schema change in `db.transaction(..)`
  does not make it atomic. That is the server's rule, not the driver's.
- `OFFSET` without `LIMIT` is not grammatical; the builder supplies an unbounded `LIMIT` when
  you set only an offset. `FULL OUTER JOIN` has no MySQL spelling, and the builder rejects it
  rather than emitting something that will not parse; `NULLS FIRST`/`NULLS LAST` are emulated by
  ordering on `expr IS NULL` first.
- MySQL identifiers are quoted with backticks and a backtick inside one is doubled, so a table
  or column with an unusual name round-trips through DDL.
- `push` is not transactional, and a failed statement leaves the earlier ones applied. On the
  native driver every statement of one push still runs on one connection, so per-session state
  such as `sql_mode` is consistent across the list.
