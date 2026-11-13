# Connect to SQLite

`besu-sqlite` ships two drivers for the same dialect:

| Driver | Feature | Connection | Default |
| --- | --- | --- | --- |
| `Sqlx` | `sqlx` (on by default) | a `sqlx::SqlitePool` | yes |
| `Native` | `native` | one `rusqlite::Connection` on its own thread | no |

The dialect, the DDL and introspection are shared. The choice is only about how a statement
reaches the database, so the same typed query runs on either. `native` is additive: enabling it
does not change what `Database` or `Sqlx` mean.

## A file, with sqlx

```rust
use besu_sqlite::Sqlx;

let db = besu::Database::new(Sqlx::new("app.db").await?);
```

`Sqlx::new` takes any `AsRef<Path>`, creates the file if it does not exist, enables foreign
keys and switches the journal to WAL. The path is a path, not a URL: `"app.db"`, not
`"sqlite://app.db"`.

## A connection string, with sqlx

```rust
let db = besu::Database::new(Sqlx::connect("sqlite://app.db").await?);
```

`Sqlx::connect` parses a URL, then applies `create_if_missing(true)` and `foreign_keys(true)`
on top of whatever the URL said. It does not change the journal mode, so WAL is only on when
you asked for it in the URL or used `Sqlx::new`.

## In memory, with sqlx

```rust
let db = besu::Database::new(Sqlx::memory().await?);
```

`Sqlx::memory()` opens `:memory:` and is the right choice for tests. Because `:memory:` lives
for exactly one connection, the driver pins the pool to a single connection and disables the
idle timeout and maximum lifetime; a pool of several connections would give each statement its
own empty database.

## Your own pool

`from_pool` takes an already-built `SqlitePool`, which is the only way to configure timeouts
and connection limits. This needs `sqlx` as a direct dependency:

```toml
[dependencies]
sqlx = { version = "0.8", default-features = false, features = ["sqlite", "runtime-tokio"] }
```

```rust
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

let pool = SqlitePoolOptions::new()
    .max_connections(4)
    .acquire_timeout(std::time::Duration::from_secs(5))
    .connect_with(
        SqliteConnectOptions::new()
            .filename("app.db")
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal),
    )
    .await?;

let db = besu::Database::new(besu_sqlite::Sqlx::from_pool(pool));
```

Besu's own options are not merged into this pool: WAL is not enabled unless you set it here,
exactly as with `Sqlx::connect`. sqlx already enables foreign keys on every connection it
opens, so the pragma above is documentation as much as configuration.

## The pool underneath

```rust
let pool = db.driver().pool();
```

`pool()` is there for anything Besu does not cover — closing the pool at shutdown, or handing
a connection to a library that wants one. `db.driver()` is the `Sqlx`, so `pool()` is one
method call away.

## The native driver

Enable the feature; it does not replace the sqlx driver.

```toml
[dependencies]
besu-sqlite = { version = "0.1", features = ["native"] }
```

```rust
use besu_sqlite::Native;

let db = Native::open("app.db")?.database();
let db: besu_sqlite::NativeDatabase = Native::in_memory()?.database();
```

There are three constructors and they are synchronous, because opening a file is synchronous:

- `Native::open(path)` (or its alias `Native::new(path)`) creates the file if it does not
  exist, enables foreign keys, sets a five-second busy timeout and switches a file database to
  WAL.
- `Native::in_memory()` opens `:memory:`. One connection means one database, so every clone of
  the driver sees the same tables; this is the driver to use in tests.
- `Native::from_connection(connection)` takes a `rusqlite::Connection` you configured
  yourself. It is moved to the driver's thread and cannot be used afterwards.

`Native::database()` is a convenience for `besu::Database::new(Native::open(path)?)`.

### How it works

`rusqlite` is synchronous, so `Native` owns a thread with the `Connection` on it. A query
encodes its parameters once, sends the SQL text, those owned values and a oneshot response
channel to the thread, and awaits the reply. The thread executes requests in order. This is one
thread per driver and a channel round trip per statement, not a blocking-pool task per
statement.

The thread shuts down when the last clone of `Native` is dropped: the request channel closes,
the worker leaves its loop, and the drop waits for it to finish.

Because there is one connection, there is one transaction context. `begin` sends `BEGIN` and
gets a transaction id; every query inside the transaction carries that id so the worker can
check it is still the open transaction. `commit` and `rollback` consume the handle, and a
handle dropped without either rolls back. Nested `tx.transaction(..)` calls are savepoints,
the same as on sqlx.

### Limitations

- **No pool.** Every clone shares the one connection, so statements are serialised. A second
  `BEGIN` while a transaction is open fails, because SQLite does not nest transactions on one
  connection.
- **One transaction at a time.** A query sent through the `Database` while a transaction is
  open runs on the same connection, inside that transaction. A pool would have given it a
  different connection; this driver does not pretend otherwise.
- **Streaming collects.** `query_stream` returns the rows of one response rather than holding
  a cursor open across awaits. A cursor would borrow the connection and block every other
  statement, so the rows are collected and then streamed from memory. The `RowStream` contract
  is still met because the rows are owned.
- **`push` has one connection, not atomicity.** SQLite's table rebuild toggles
  `PRAGMA foreign_keys`, which is per-connection state; the native driver runs every schema
  statement on its one connection, in order. A failure part-way still leaves the earlier
  statements applied, exactly as with sqlx. Only migrations run in a transaction.

## Transactions (either driver)

```rust
let mut tx = db.begin().await?;
tx.insert(users).value(users.insert("a@example.com", "A", 21)).await?;
tx.commit().await?;
```

`begin` borrows the database and runs every statement on one connection; `commit` and
`rollback` consume the handle. `db.transaction(..)` wraps the same thing in a closure that
commits on `Ok` and rolls back on `Err`. Savepoints are `tx.savepoint("name", ..)`.

A schema change executes all of its statements on one connection too, because SQLite's table
rebuild toggles `PRAGMA foreign_keys`, and a pragma is per-connection.

## No connection at all

```rust
let db = besu::dry::<besu_sqlite::SQLite>();

let sql = db.select(users).filter(users.age.gte(18)).to_sql()?;
assert!(sql.sql().contains("WHERE (\"users\".\"age\" >= ?)"));
```

`besu::dry` builds a `Database<DryDriver<SQLite>>`: the typed builder and an exact `to_sql()`,
with no file, no pool and no runtime. Executing one returns an error that says to call
`to_sql` instead.

## Gotchas

- SQLite itself leaves foreign-key enforcement off. sqlx turns it on for every connection it
  opens, the native driver sets the pragma on its connection, and Besu sets it explicitly —
  but a connection made outside either driver (the `sqlite3` shell, for instance) does not
  inherit it.
- `Sqlx::new` and `Native::open` create the file, so a typo in the path creates a new empty
  database rather than failing. A missing table is the first symptom.
- `Sqlx::memory()` gives each `Database` its own private database; there is no way to share one
  between connections. Two `Sqlx::memory()` calls are two unrelated databases.
- `Native::in_memory()` is one database per `Native`, but every clone of that `Native` shares
  it. Two calls are still two databases.
- A pool over a file and a pool over `:memory:` are not interchangeable: with `:memory:` every
  connection would be empty, which is why the sqlx constructor forces one connection and
  ignores `max_connections`.
- WAL is a property of `Sqlx::new` and `Native::open`, not of the dialect.
  `Sqlx::connect("sqlite://app.db")` leaves the journal mode at SQLite's default (rollback
  journal), and `Native::from_connection` leaves it as you configured it.
- `push` executes each statement separately, so a failure part-way leaves the earlier
  statements applied. Only migration files run inside a transaction.
- The `native` feature compiles SQLite from source (rusqlite's `bundled` feature), so the
  first build is slower and the binary does not depend on a system SQLite. It is also why
  rusqlite is pinned to the release that shares `libsqlite3-sys` 0.30 with sqlx 0.8: the two
  drivers can be enabled together in one build.
