# Contributing

## Layout

```
besu-core     traits and SQL primitives, no database and no query API
besu          the query builders and expression DSL
besu-schema   the schema IR, diffing, DDL and migration runner
besu-macros   #[derive(Table)]
besu-sqlite   \
besu-postgres  > a dialect, a driver, DDL and introspection per database
besu-mysql    /
cli           `besu push|pull|generate|migrate`
examples      runnable examples, one per database
```

The dependency graph only ever points one way:

```
besu-core ──┬─▶ besu ──▶ besu-sqlite / besu-postgres / besu-mysql ──▶ cli
            └─▶ besu-schema ─▶ (besu, drivers)
```

If you find yourself wanting to add a dependency from `besu-core` to anything in this
workspace, the abstraction is wrong — say so in an issue instead.

## Running the tests

SQLite tests need nothing. PostgreSQL and MySQL tests need a server:

```sh
docker run -d --name besu-postgres -e POSTGRES_PASSWORD=besu -e POSTGRES_USER=besu \
  -e POSTGRES_DB=besu -p 55432:5432 postgres:17-alpine

docker run -d --name besu-mysql -e MYSQL_ROOT_PASSWORD=besu -e MYSQL_DATABASE=besu \
  -p 53306:3306 mysql:8.4

cargo test --workspace
```

The connection strings are hard-coded in the tests because they are only ever run against
these containers.

## Design rules

A few rules are load-bearing; please do not break them without a discussion in an issue.

1. **The query you write is the SQL that runs.** No rewriting, no query planning, no hidden
   extra statements. If a feature cannot be expressed without changing the shape of the SQL,
   it belongs in an issue, not in a clever optimisation.
2. **Parameters are bound, never interpolated.** There is deliberately no API that formats a
   value into SQL. `sql!` is the escape hatch and it still binds.
3. **Errors the compiler can catch should not be runtime errors.** Column types, table
   membership, `WHERE`-less updates and `sql!` arity are all checked at compile time; new
   features should follow suit where it is at all possible.
4. **Every allocation should be justifiable.** A query builds its SQL exactly once, and a
   parameter is encoded exactly once. Cloning a string per parameter is a bug.
5. **Drivers are dumb.** Anything about *what* SQL to run belongs in the dialect, so the same
   code path serves a driverless test and a production pool.

## Adding a driver

Implement three things and you have a database:

1. `besu_core::Dialect` — placeholders, identifier quoting, capability flags and the type map.
2. `besu_core::Driver` (and `TransactionDriver` if you can do transactions) — execution and
   row access, plus `Decode` impls for the primitive types.
3. `besu::SchemaDriver` and `besu_schema::Ddl` — introspection and DDL, if you want
   migrations.

`besu-sqlite` is the smallest complete example; read it first.
