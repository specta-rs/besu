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

## The studio frontend

`besu-studio/web` is a Vite + SolidJS + Tailwind application. Its build is committed to
`web/dist` and embedded into the binary by `besu-studio/build.rs`, which is what keeps
`cargo build` free of a Node toolchain and makes the published crate serve a working UI.

```sh
pnpm --dir besu-studio/web install
pnpm --dir besu-studio/web dev     # http://localhost:5173, proxying /api to a running studio
pnpm --dir besu-studio/web build   # rewrites web/dist — commit the result
```

The `studio` CI job fails when `web/dist` is stale, so a change that touches `web/src` and not
the bundle is caught in review rather than at release.

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

Trait changes are the expensive part of this rulebook: `Driver`, `Executor` and `Ddl` are public
API, and adding a method is a breaking change for every driver, so open an issue before starting
one of those. A new expression, a new `Change` variant or a CLI improvement is much easier to
land.

## Releasing

Publishing uses crates.io trusted publishing: the `release` workflow exchanges its GitHub OIDC
identity for a short-lived crates.io token, so **no `CARGO_REGISTRY_TOKEN` is stored** in this
repository or in Actions secrets.

**One-time setup** — a crate owner adds a trusted publisher in the crates.io UI (the crate's
**Settings → Trusted Publishing**) for every published crate, and enables **Require trusted
publishing** so a long-lived token cannot be used instead. These values must match
`.github/workflows/release.yml` exactly, or the token exchange is refused:

| Field | Value |
| --- | --- |
| Owner | `specta-rs` |
| Repository | `besu` |
| Workflow filename | `release.yml` |
| Environment | `release` |

The workflow publishes the whole workspace, so each of these crates needs its own trusted
publisher:

```
besu-core, besu-schema, besu-macros, besu,
besu-sqlite, besu-postgres, besu-mysql, besu-any,
besu-cli, besu-studio
```

`cargo publish --workspace` works out the order from the dependency graph, so the list does not
have to be kept sorted by hand. The examples are `publish = false` and are skipped.

### The first release of a crate

A trusted publisher is a setting on a crate that already exists, so a crate cannot be published
for the first time through this workflow. The first version has to go up with an API token — a
scoped token from **Account Settings → API Tokens**, used once and then revoked:

```sh
CARGO_REGISTRY_TOKEN=... cargo publish --workspace --locked
```

`--workspace` publishes in dependency order, so one command releases every crate including the new
ones. Afterwards, add the trusted publisher above to each crate that just appeared and enable
**Require trusted publishing**; from then on the token is refused and only `release.yml` can
publish.

All ten crates were released once by hand at `0.1.0` — the last time a token is used — and every
one of them now carries the trusted publisher above with **Require trusted publishing** enabled.
A version tag is therefore the only way to publish from here on.

**Cutting a release:**

1. Bump the `version` of every workspace crate and merge to `main`. The tag names the `besu`
   crate version; the workspace uses `version.workspace = true`, so one edit to the root
   `Cargo.toml` moves all of them.
2. Make sure the release is what CI tested:

   ```sh
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets --all-features -- -D warnings
   cargo test --workspace --all-features
   RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --exclude besu-cli
   ```

3. Tag the merge commit and push the tag:

   ```sh
   git tag v0.1.0
   git push origin v0.1.0
   ```

The `release` workflow verifies that the tag matches `besu`'s version, then checks and tests the
whole workspace — with PostgreSQL and MySQL service containers, because two of the drivers'
suites need a live server — publishes every crate in dependency order, and creates the GitHub
release.

If the token exchange fails, the four fields above do not match the trusted publisher for one
of the crates, or that crate has not been given one yet — crates.io reports which one refused the
exchange.

## Adding a driver

Implement three things and you have a database:

1. `besu_core::Dialect` — placeholders, identifier quoting, capability flags and the type map.
2. `besu_core::Driver` (and `TransactionDriver` if you can do transactions) — execution and
   row access, plus `Decode` impls for the primitive types.
3. `besu::SchemaDriver` and `besu_schema::Ddl` — introspection and DDL, if you want
   migrations.

`besu-sqlite` is the smallest complete example; read it first.
