<div align="center">
    <picture>
        <source media="(prefers-color-scheme: dark)" srcset="assets/besu-lockup-dark.svg">
        <img src="assets/besu-lockup-light.svg" alt="Besu logo" height="150">
    </picture>
    <h1>Besu</h1>
    <p><b>A typesafe, async, database-agnostic SQL query builder for Rust</b></p>
    <a href="https://crates.io/crates/besu"><img src="https://img.shields.io/crates/v/besu?style=flat-square" alt="crates.io"></a>
    <a href="https://docs.rs/besu"><img src="https://img.shields.io/badge/docs-latest-blue?style=flat-square" alt="docs.rs"></a>
    <a href="LICENSE"><img src="https://img.shields.io/crates/l/besu?style=flat-square" alt="License"></a>
    <a href="https://github.com/specta-rs/besu/actions/workflows/ci.yml"><img src="https://github.com/specta-rs/besu/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
</div>

<br>

## Why Besu

ORMs invent a query language of their own and then spend forever translating it. The result is
familiar: the generated SQL is not the SQL you would have written, the query planner disagrees, and
the moment you need something the abstraction did not anticipate you are back to raw strings with
no checking at all.

Besu takes the opposite position. **The query you write is the SQL that runs.** There is no
translation layer to outsmart, no lazy-loading magic and no N+1 surprises — just a typed surface
over SQL that catches your mistakes before the database does.

It is a query builder, not an ORM. You keep full control of indexes, joins and the shape of every
statement. The compiler keeps you honest about column names, column types, nullability and which
tables are actually in the query.

## What it checks

```rust
db.select(users).filter(users.age.gt(18))          // OK
db.select(users).filter(users.age.gt("eighteen"))  // wrong column type
db.select(users).filter(posts.title.eq("Hello"))   // `posts` was never joined
db.select(users).pick(users.missing)               // no such column
```

And the mistakes that quietly destroy data in production:

```rust
db.delete(users).filter(users.id.eq(1)).await?;    // fine
db.delete(users).await?;                           // does not compile
db.delete(users).all_rows().await?;                // says what it does
```

Every one of those is a compile error with a readable message, not a runtime error, not a silently
empty `WHERE` clause and not a stringly-typed panic.

## What it looks like

```rust
use besu::prelude::*;

#[derive(Table)]
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

// SELECT "users"."id", ... FROM "users" WHERE ("users"."age" > $1) ORDER BY "users"."name" ASC
let adults: Vec<User> = db
    .select(users)
    .filter(users.age.gt(18))
    .order_by(users.name.asc())
    .limit(50)
    .await?;

// INSERT INTO "users" ("email", "name", "age") VALUES ($1, $2, $3)
// RETURNING "users"."id", "users"."email", ...
let created: User = db
    .insert(users)
    .value(users.insert("oscar@example.com", "Oscar", 21))
    .returning(users)
    .await?
    .into_iter()
    .next()
    .expect("RETURNING always yields a row");

// SELECT ... FROM "users" LEFT JOIN "posts" ON ("users"."id" = "posts"."author_id")
let feed: Vec<(User, Option<Post>)> = db
    .select(users)
    .left_join(posts, eq(users.id, posts.author_id))
    .filter(posts.published)
    .await?;
```

The table handle is generated from the struct: `users` is a `const` value whose fields are
typed column references. `users.id` costs nothing to copy, reads like the SQL it produces, and
carries the column's name, type and nullability in the type system.

## What's inside

| Crate | Role |
| ----- | ---- |
| [`besu-core`](besu-core) | The vocabulary: `Dialect`, `Driver`, `Sql`, `Value` and row decoding. No query API and no database, so SQL can be built with nothing attached. |
| [`besu`](besu) | The query builders and expression DSL: select, insert, update, delete, joins, upserts, aggregates, transactions and raw SQL. |
| [`besu-schema`](besu-schema) | One serialisable schema IR shared by migrations, introspection and DDL generation. |
| [`besu-macros`](besu-macros) | `#[derive(Table)]`. |
| [`besu-sqlite`](besu-sqlite) · [`besu-postgres`](besu-postgres) · [`besu-mysql`](besu-mysql) | Dialects, drivers, DDL and introspection for each database. |
| [`besu-any`](besu-any) | `AnyDatabase`: one type over all three backends, chosen from a connection string. |
| [`besu-cli`](cli) | `besu push`, `besu pull`, `besu generate` and `besu migrate`. |
| [`besu-studio`](besu-studio) | The library behind `besu studio`, a development-only schema and data browser. |

The layers are deliberately independent. `besu-core` knows nothing about the query API, `besu`
knows nothing about sqlx, and each driver knows nothing about the others. Use the whole stack, or
take just the layer you need — including writing your own driver against `besu-core`.

## Driverless SQL

Because a dialect can render a query with no connection, you can hand-check the SQL in a unit test:

```rust
let db = besu::dry::<besu_postgres::Postgres>();

let sql = db.select(users).filter(users.age.gt(18)).to_sql()?;
assert!(sql.sql().ends_with(r#"WHERE ("users"."age" > $1)"#));
```

Awaiting a query built this way returns an error rather than panicking, so a driverless build can
still be exercised end to end.

## Performance

Building a query should cost almost nothing, because the database is the slow part. A
statement is one `String` of SQL and one `Vec` of bound values, and the builder adds only the
handful of small vectors that hold its clauses:

| Query | Time |
| ----- | ---- |
| `select` with one filter | 157 ns |
| `select` with three filters, `ORDER BY` and `LIMIT` | 318 ns |
| `select` with a projection and `COUNT(*)` | 152 ns |
| `insert` one row | 193 ns |
| `insert` one hundred rows | 10.5 µs |
| `update ... set ... where` | 246 ns |

Measured with `cargo bench -p besu` on a release build, no database involved — that is the
cost Besu adds on top of the driver. A one-filter `select` allocates four times: the clause
vector, the statement text, the argument vector, and one clone that `to_sql` makes so the
builder stays usable. Executing a query moves its values instead and allocates three.

Rows are decoded straight from the driver's row type into your struct, with no per-row map
and no intermediate `serde` pass, and `db.stream::<User>(&sql)` decodes one row at a time when
a result set is too large to hold. Each driver carries its own criterion bench; see
[CONTRIBUTING](CONTRIBUTING.md#benchmarks).

## Beyond the basics

- **Streaming.** `db.stream::<User>(&sql)` decodes row by row, so a million-row scan is a
  million rows of work rather than a million rows of memory.
- **Relations.** `select(users).with(has_many(users.id, posts.author_id))` loads the children
  in one extra, visible statement rather than an invisible join or an N+1.
- **Transactions and savepoints.** `db.transaction(..)` and `tx.transaction(..)`, which every
  database spells as a savepoint.
- **Full-text search and JSON.** `posts.title.matches("rust")` and
  `json_text(documents.body, "user.name")`, each rendered natively per dialect.
- **Observability.** `besu::logged(driver)` adds tracing, timing and a slow-query warning.
- **Table prefixes.** `Database::prefixed(driver, "tenant_42_")` for tenants sharing one
  physical database.

## Migrations

The schema you declare in code and the schema a live database has are the *same type*. That makes
`push` a two-line operation and drift impossible to miss:

```rust
// Creates the tables, or applies only the difference if they already exist.
let result = db.push([users.def(), posts.def()]).await?;
```

Destructive changes — dropping a column or a table — are refused unless you call
`push_destructive`. For production, generate SQL files and run them in order with a journal table:

```rust
db.migrate(&Migrator::from_dir("migrations")?).await?;
```

## Documentation

The README is an overview. The full documentation lives in [`docs/`](docs/README.md).

- [Get started with SQLite](docs/get-started/sqlite.md) · [PostgreSQL](docs/get-started/postgresql.md) · [MySQL](docs/get-started/mysql.md)
- [Schema declaration](docs/schema/declaration.md) and [column types](docs/schema/column-types/postgresql.md)
- [Select](docs/queries/select.md), [insert](docs/queries/insert.md), [update](docs/queries/update.md), [delete](docs/queries/delete.md)
- [Operators](docs/queries/operators.md), [joins](docs/queries/joins.md) and [the `sql!` operator](docs/queries/sql.md)
- [Transactions](docs/advanced/transactions.md), [set operations](docs/advanced/set-operations.md) and [subqueries](docs/advanced/subqueries.md)
- [Migrations](docs/migrations/overview.md)
- [Drizzle parity](docs/parity.md), [performance](docs/performance.md) and [writing a driver](docs/drivers.md)
- [Architecture](docs/architecture.md), [comparison](docs/comparison.md) and [roadmap](docs/roadmap.md)

API reference is on [docs.rs](https://docs.rs/besu).

## Inspiration

Besu is inspired by [Drizzle ORM](https://orm.drizzle.team) — specifically its insistence that a
query builder should express raw SQL rather than replace it — and by elements of
[SQLx](https://github.com/launchbadge/sqlx) and
[Prisma Client Rust](https://prisma.brendonovich.dev). Earlier iterations of this idea in this
organisation were [`isomer`](https://github.com/specta-rs/isomer),
[`endure`](https://github.com/specta-rs/endure) and [`sqr`](https://github.com/specta-rs/sqr);
their useful parts live on here.

## Contributing

Development and release instructions are in [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Licensed under the [MIT License](LICENSE).
