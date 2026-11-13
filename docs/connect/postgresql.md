# Connect to PostgreSQL

`besu_postgres` ships two drivers for the same dialect:

- `Sqlx` wraps a sqlx `PgPool`. It is behind the default `sqlx` feature and stays the
  default when `native` is also enabled.
- `Native` wraps a `deadpool-postgres` pool of `tokio-postgres` connections. It is behind
  the additive `native` feature and has no sqlx in its path.

A URL is all either one needs. Everything below applies to both unless a section says
otherwise.

## A URL

```rust
use besu_postgres::Sqlx;

let db = besu::Database::new(
    Sqlx::connect("postgres://besu:besu@localhost:5432/besu").await?,
);
```

`Sqlx::new` is an alias for `Sqlx::connect`, so either name works. The URL carries TLS mode
and the application name in its query string, which is how sqlx expects them:

```rust
let db = besu::Database::new(
    Sqlx::connect("postgres://besu:besu@db.internal/app?sslmode=require").await?,
);
```

Reading it from the environment is the usual shape, and it keeps the password out of the
binary:

```rust
let url = std::env::var("DATABASE_URL")?;
let db = besu::Database::new(Sqlx::connect(&url).await?);
```

## The native driver

Enable the feature and use the same URL:

```toml
[dependencies]
besu-postgres = { version = "0.1", features = ["native"] }
```

```rust
use besu_postgres::Native;

let db = besu::Database::new(
    Native::connect("postgres://besu:besu@localhost:5432/besu").await?,
);
```

`Native::new` is the alias, and `Native::database()` is shorthand for wrapping the driver in
a `besu::Database`. The constructor opens one connection before returning, so a typo in the
URL fails there rather than at the first query.

The pool holds at most ten connections and is shared by every clone of the driver.
`Native::pool()` hands back the `deadpool_postgres::Pool`, and `Native::from_pool` wraps one
you configured yourself:

```rust
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};
use besu_postgres::Native;

let config: tokio_postgres::Config = url.parse()?;
let manager = Manager::from_config(
    config,
    tokio_postgres::NoTls,
    ManagerConfig { recycling_method: RecyclingMethod::Fast },
);
let pool = Pool::builder(manager)
    .max_size(20)
    .runtime(Runtime::Tokio1)
    .build()?;

let db = besu::Database::new(Native::from_pool(pool));
```

Unlike the sqlx driver, the native one does not expose TLS yet: it uses
`tokio_postgres::NoTls`. A URL that requires TLS fails to connect rather than silently
falling back to plaintext. TLS is a real API decision — a feature, a connector argument and
an `sslmode` story — and is deliberately left for a later change.

## Your own pool

`Sqlx::from_pool` is the sqlx escape hatch. It is the only way to change the sqlx connection
limit, the acquire timeout or the idle timeout, and it needs `sqlx` as a direct dependency:

```toml
[dependencies]
sqlx = { version = "0.8", default-features = false, features = ["postgres", "runtime-tokio"] }
```

```rust
use sqlx::postgres::PgPoolOptions;

let pool = PgPoolOptions::new()
    .max_connections(20)
    .acquire_timeout(std::time::Duration::from_secs(5))
    .connect("postgres://besu:besu@localhost:5432/besu")
    .await?;

let db = besu::Database::new(besu_postgres::Sqlx::from_pool(pool));
```

## The pool underneath

```rust
let pool = db.driver().pool();
```

`pool()` is the escape hatch: closing it at shutdown, or passing it to a library that wants
the pool directly. `db.driver()` returns the driver, so the pool is one call away. The type
is `sqlx::PgPool` for `Sqlx` and `deadpool_postgres::Pool` for `Native`.

## Transactions

```rust
let mut tx = db.begin().await?;
tx.insert(users).value(users.insert("a@example.com", "A", 21)).await?;
tx.commit().await?;
```

`begin` borrows the database and runs every statement on one connection. `db.transaction(..)`
commits on `Ok` and rolls back on `Err`, and `tx.savepoint("name", ..)` nests inside an open
transaction. A `Transaction` is an executor, so the same builders work through `tx`.

On the native driver, `begin` checks a connection out of the pool and issues `BEGIN` itself.
Dropping a `Transaction` without committing it detaches that connection instead of returning
it to the pool: the socket closes, PostgreSQL rolls the transaction back, and the next
borrower is never handed a connection that is still inside one. The cost is that an
uncommitted transaction destroys one pooled connection.

## No connection at all

```rust
let db = besu::dry::<besu_postgres::Postgres>();

let sql = db
    .select(users)
    .filter(users.age.gte(18))
    .limit(50)
    .to_sql()?;

assert!(sql.sql().contains(r#"WHERE ("users"."age" >= $1)"#));
```

That renders the exact statement the driver would send, including the `/* besu:ii */`
parameter-shape suffix, with no server and no runtime.

## Gotchas

- Every statement with parameters carries a `/* besu:... */` comment describing the bound
  value shapes. PostgreSQL fixes parameter types at prepare time; on the sqlx driver the
  statement cache is keyed by SQL text, so two calls with the same text but different
  parameter types would otherwise hit the same entry with the wrong types. On the native
  driver the server infers a type per placeholder from the statement, and the encoder writes
  the value in that inferred type. An assertion on the whole string must include the comment.
- `push` is not transactional. A failed statement leaves the earlier ones applied; use
  `besu generate` and `besu migrate` for production changes.
- The default schema is implicit. Introspection reports a table in `current_schema()` without
  its schema, so `#[besu(schema = "public")]` on a table in `public` makes the declared table
  look like a different one and reports drift.
- Besu never runs `CREATE SCHEMA` or `CREATE DATABASE`; both have to exist.
- A table in another schema has `(schema, name)` as its identity, so `app.users` and `users`
  are two tables that can both exist.
- `push` manages only the schemas the declared schema mentions. A table in some other schema
  is left alone rather than reported as a drop.
- The default connection limit is ten. A service that opens several driver values opens
  several pools, each with ten connections.
- The native driver has no TLS. A connection string that asks for `sslmode=require` is
  rejected by the server rather than downgraded, but there is no way to complete the
  handshake until a TLS feature exists.
- Streaming on the native driver keeps a pooled connection checked out until the stream (and
  every row in it) is dropped. A leaked stream is a leaked connection; the `Rows` type from
  `Database::stream` makes the end of the stream explicit.

