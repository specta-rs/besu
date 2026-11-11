# Besu documentation

Besu is a typesafe, async, database-agnostic SQL query builder for Rust. It is not an ORM:
every query you write maps onto exactly the SQL you would have written by hand, and the
compiler checks the parts of that SQL it can see — column names, column types, nullability,
which tables are in the query, and whether a bulk write was intentional.

The documentation follows the same path as the API: define a table from a plain struct, build
a query out of typed columns, render it to SQL, and run it against a driver. The later pages
cover the schema IR that powers migrations and the traits you implement to add a database.

## Start here

- [Getting started](guide/getting-started.md) — install the crates, define a table, run your first query
- [Defining a schema](guide/schema.md) — every `#[derive(Table)]` attribute, the type mappings, nullability, defaults, indexes and foreign keys

## Guide

- [Selecting data](guide/select.md) — filtering, ordering, paging, projections, joins, grouping and aggregates
- [Inserting data](guide/insert.md) — `value`/`values`, the generated insert struct, `Set`, `RETURNING` and upserts
- [Updating and deleting](guide/update-delete.md) — the `all_rows()` safety mechanism, `set` versus `set_expr`, `RETURNING`
- [Raw SQL](guide/raw-sql.md) — the `sql!` macro, placeholder rewriting, compile-time arity checking and escape hatches
- [Transactions](guide/transactions.md) — `begin`/`commit`/`rollback` and the `transaction()` helper
- [Migrations](guide/migrations.md) — the schema IR, `push` versus `push_destructive`, generated files and the migration runner
- [Writing a driver](guide/drivers.md) — `Dialect`, `Driver`, `TransactionDriver`, `SchemaDriver`, `Ddl` and `Decode`

## Reference

- [Architecture](architecture.md) — the crate graph, where the seams are, and how a query becomes SQL
- [Comparison](comparison.md) — how Besu differs from Diesel, SeaORM, sqlx and Drizzle
- [Roadmap](roadmap.md) — what is missing, in priority order, and what each item would take

## Crates

| Crate | Role |
| --- | --- |
| [`besu-core`](../besu-core) | The vocabulary: `Dialect`, `Driver`, `Executor`, `Sql`, `Value` and row decoding. No query API and no database, so SQL can be built with nothing attached. |
| [`besu`](../besu) | The query builders and expression DSL: select, insert, update, delete, joins, upserts, aggregates, transactions and raw SQL. |
| [`besu-schema`](../besu-schema) | One serialisable schema IR shared by migrations, introspection and DDL generation. |
| [`besu-macros`](../besu-macros) | `#[derive(Table)]`. |
| [`besu-sqlite`](../besu-sqlite) · [`besu-postgres`](../besu-postgres) · [`besu-mysql`](../besu-mysql) | Dialects, drivers, DDL and introspection for each database. |
| [`besu-cli`](../cli) | `besu pull`, `besu push`, `besu generate` and `besu migrate`. |

## How to read a page

Each guide page ends with **What is checked** or **Gotchas**. Those sections are the honest
part: what the compiler proves, what it cannot prove, and which mistakes are still possible.
If a feature is not implemented, the page says so and points at the [roadmap](roadmap.md)
rather than pretending otherwise.

Snippets use SQLite because it needs no server. The same query code works with any driver;
only the connection setup changes.
