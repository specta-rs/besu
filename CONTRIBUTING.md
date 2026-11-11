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

## Benchmarks

`cargo bench -p besu` measures the query builder. It uses a dry database and a hand-written
dialect, so it needs no server and the numbers are the cost Besu adds on top of a driver.

Each driver has its own bench, which needs its server:

```sh
BESU_SQLITE_BENCH_URL=/tmp/besu-bench.db cargo bench -p besu-sqlite
BESU_POSTGRES_BENCH_URL=postgres://besu:besu@localhost:55432/besu cargo bench -p besu-postgres
BESU_MYSQL_BENCH_URL=mysql://root:besu@localhost:53306/besu cargo bench -p besu-mysql
```

Without the variable the database half of the bench prints a line and registers nothing, so
`cargo bench` still works on a machine with no containers.

A performance claim in a pull request should come with `cargo bench` numbers from before and
after, and with the allocation counts if it is about allocations — `dhat` in a throwaway
crate is enough, do not add it to the workspace.

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

## Releasing

The crates must be published in dependency order, because each one depends on the ones
before it and `cargo package` verifies against the registry rather than the workspace:

```
besu-core -> besu-schema -> besu-macros -> besu -> besu-sqlite / besu-postgres / besu-mysql
```

Run `cargo publish -p <crate> --dry-run` for the first two, then `cargo publish` for each in
turn. `besu-cli` ships a binary called `besu`, so it is published last. Bump every version
together — the workspace uses `version.workspace = true`, so one edit to the root
`Cargo.toml` moves all of them.

Before publishing, check:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --exclude besu-cli
```

## Adding a driver

Implement three things and you have a database:

1. `besu_core::Dialect` — placeholders, identifier quoting, capability flags and the type map.
2. `besu_core::Driver` (and `TransactionDriver` if you can do transactions) — execution and
   row access, plus `Decode` impls for the primitive types.
3. `besu::SchemaDriver` and `besu_schema::Ddl` — introspection and DDL, if you want
   migrations.

`besu-sqlite` is the smallest complete example; read it first.
