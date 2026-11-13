# Besu documentation

Besu is a typesafe, async, database-agnostic SQL query builder for Rust. It is not an ORM:
every query you write maps onto exactly the SQL you would have written by hand, and the
compiler checks the parts it can see — column names, column types, nullability, which tables
are in the query, and whether a bulk write was intentional.

## Fundamentals

- [Overview](overview.md) — what Besu is, and what it deliberately is not
- [Get started with SQLite](get-started/sqlite.md)
- [Get started with PostgreSQL](get-started/postgresql.md)
- [Get started with MySQL](get-started/mysql.md)
- [Schema declaration](schema/declaration.md) — `#[derive(Table)]` and every attribute
- [Relations](schema/relations.md) — relation values, many-to-many and nested loading without an N+1
- [Connect](connect/overview.md) — a driver, a pool, a database
- [Queries and CRUD](queries/select.md) — the shape of every statement
- [Migrations](migrations/overview.md) — the schema IR, `push`, `generate`, `migrate`

## Manage schema

- Column types: [PostgreSQL](schema/column-types/postgresql.md) · [MySQL](schema/column-types/mysql.md) · [SQLite](schema/column-types/sqlite.md)
- [Indexes and constraints](schema/indexes-constraints.md)
- [Views](schema/views.md)
- [Sequences](schema/sequences.md) — PostgreSQL
- [Table schemas](schema/schemas.md) — PostgreSQL
- [Extensions](schema/extensions.md) — PostgreSQL
- [Custom types](advanced/custom-types.md)

## Access your data

- [Select](queries/select.md) · [Insert](queries/insert.md) · [Update](queries/update.md) · [Delete](queries/delete.md)
- [Filter and conditional operators](queries/operators.md)
- [Joins](queries/joins.md)
- [Aliases](queries/aliases.md)
- [The `sql!` operator](queries/sql.md)
- [Query utilities](queries/query-utils.md)
- [SQL comments](queries/comments.md)
- [Set operations](advanced/set-operations.md)
- [Subqueries](advanced/subqueries.md)

## Migrations

- [Migrations overview](migrations/overview.md)
- [`besu generate`](migrations/generate.md) · [`besu migrate`](migrations/migrate.md) · [`besu push`](migrations/push.md) · [`besu pull`](migrations/pull.md)
- [`besu studio`](migrations/studio.md)
- [Custom migrations](migrations/custom.md)
- [Configuration](migrations/config.md)

## Advanced

- [Transactions](advanced/transactions.md)
- [Generated columns](advanced/generated-columns.md)
- [Batch API](advanced/batch-api.md)
- [Dynamic query building](advanced/dynamic-query-building.md)
- [CTEs](advanced/ctes.md)
- [Seeding](advanced/seeding.md)
- [Cache](advanced/cache.md)
- [Read replicas](advanced/read-replicas.md)

## Reference

- [Performance](performance.md) — what a query costs, with numbers
- [Architecture](architecture.md) — the crate graph and how a query becomes SQL
- [Writing a driver](drivers.md) — `Dialect`, `Driver`, `SchemaDriver`, `Ddl`, `Decode`
- [Comparison](comparison.md) — Besu against Diesel, SeaORM, sqlx and Drizzle
