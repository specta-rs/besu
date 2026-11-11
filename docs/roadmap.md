# Roadmap

Besu is early. This page lists what is missing, in rough priority order, and what each item
would actually require. The order is a judgement about impact versus cost, not a promise.

| # | Item | Why it matters | Rough size |
| --- | --- | --- | --- |
| 1 | [Custom column types](#custom-column-types) | `DECIMAL`, enums and domain types are only partly reachable | Medium |
| 2 | [wasm support](#wasm-support) | The builder compiles for wasm32; execution needs a browser driver | Large |

## Custom column types

Tracked as [#55](https://github.com/specta-rs/besu/issues/55).

`#[besu(sql_type = "...")]` now names a `SqlType` directly, so a sized `VARCHAR`, an exact
`DECIMAL` and a user-defined marker type can all be declared. What is still missing is a
first-class decimal value type: `Decimal`'s Rust type is a `String`, which SQLite handles by
storing text, but PostgreSQL and MySQL return `NUMERIC` and no driver decodes that into a
string yet.

`TypeDef::Custom(Cow<'static, str>)` already exists and introspection uses it for native types
Besu does not model. The derive does not: a field type outside the built-in list is a compile
error, so `DECIMAL`, PostgreSQL enums and domain types cannot be declared.

What it would take:

- A derive attribute such as `#[besu(sql_type = "decimal(10,2)")]`, or a `ColumnType` trait
  mapping a Rust type to a `SqlType` plus `IntoSqlValue`.
- `SqlType` implementations for `Decimal`, `Char` and `VarChar` that a field can name. The
  types exist; the derive cannot produce them.
- DDL for `TypeDef::Custom` that round-trips (the dialects already pass the string through in
  most cases).
- A `Decode` story: a user type still has to decode from a driver row, so either the user
  implements `Decode` per driver or the mapping goes through an existing primitive.

## wasm support

The builder half is done: `besu-core` and `besu` compile for `wasm32-unknown-unknown`, so a
web front end can build a query and render the exact SQL a server will run. The `wasm` CI job
keeps it that way and `besu/tests/dry.rs` pins the SQL. See
[Compiling for wasm32](architecture.md#compiling-for-wasm32) for what does and does not work.

Executing a query in a browser is still open, and it is not a matter of a flag. The drivers
are built on sqlx, which is built on tokio, which reaches for sockets this target does not
have. What it would take:

- A new driver for a browser storage API (IndexedDB, OPFS) or an HTTP bridge to a server. It
  has to supply its own `Decode` impls, because decoding a row is the driver's job, and a
  `Dialect` that spells SQL the other end understands.
- A decision about blocking: browser storage is async but has no connection pool, while the
  `Driver` trait assumes a `Send` future and a `Send + Sync` driver. On a single-threaded wasm
  module those bounds are vacuous rather than wrong, but a driver holding JS handles is not
  `Send` in any useful sense.
- A story for the clock. `besu::logged` reads `Instant::now()` once a subscriber enables it,
  and that traps in a browser; migrations would need a timestamp that does not.

Ship the same query builder to the client and execute it on the server in the meantime; that
is the shape this half of the work was for.

## Smaller items

These are not large enough for their own section, but they are real. Each is a tracking issue.

- **[#67](https://github.com/specta-rs/besu/issues/67) — savepoints and nested
  transactions.** One level only.
- **[#68](https://github.com/specta-rs/besu/issues/68) — down migrations.** The runner is
  append-only by design.

## Recently closed

Kept here so the history is visible rather than deleted:

- **#52, eager loading and relations.** `has_many(parent_key, child_foreign_key)` plus
  `Select::with` load a has-many relation with one batched second statement, and an empty
  parent result sends no second statement at all. The parent and child columns are the whole
  descriptor, so the schema layer is unchanged. Insert-with-relations, many-to-many and nested
  relations stay open.
- **#59, `u128` support.** `Value::UInt128` / `Value::Int128`, a `U128` / `I128` `SqlType`
  and derive mapping, and encode/decode in every driver. PostgreSQL and MySQL use exact
  39-digit decimals; SQLite stores the decimal digits as `TEXT`.
- **#60, the builder on wasm32.** `besu-core` and `besu` compile for
  `wasm32-unknown-unknown`, so SQL can be generated in a browser and executed on a server.
  Execution in the browser still needs a driver of its own; see
  [wasm support](#wasm-support).
- **#63, two-sided containment.** Both operands of a comparison, every aggregate argument and
  every helper-function argument are checked now.
- **#64, SQLite column changes.** `push` rebuilds the table instead of refusing.
- **#65, observability.** `besu::logged(driver)` wraps any driver with tracing, timing and a
  slow-query warning.
- **#69, parameter limits.** `Sql` refuses a statement that exceeds the dialect's limit.
- **#70, batch insert chunking.** `Insert::values_chunked`.
- **#74, a literal `?`.** `??` is an escaped question mark, for `jsonb ? 'key'`.
- **#75, publish metadata.** Every crate has a README, keywords and categories, and the
  publish order is written down in `CONTRIBUTING.md`.
- **#76, update from model.** `Update::set_model`.
- **#66, partial indexes, checks and views.** `Index::predicate` and `Table::checks` are
  introspected, diffed and rendered, `Schema::views` round-trips through `besu pull`, and an
  object the IR cannot express (an expression index, an exclusion constraint, a trigger) is
  skipped by introspection instead of making `push` drop it. A check cannot be declared in
  Rust yet; SQLite and MySQL do not introspect one, and SQLite does not read a partial index's
  predicate, so those stay migration-only there.
- **#71, PostgreSQL composite `UNIQUE`.** A multi-column constraint is `Table::uniques`,
  added and dropped with `ALTER TABLE`, instead of an index that `DROP INDEX` could not
  remove.
- **#72, schema-qualified tables.** A table's identity is `(schema, name)`; introspection
  reports every non-system schema and the differ pairs tables by both.
- **#73, `TimeTz` and enum types.** `TypeDef::TimeTz` for PostgreSQL's `TIMETZ`, and
  `Schema::enums` is introspected, diffed and rendered as `CREATE TYPE` / `ALTER TYPE`. A
  `DECIMAL` still cannot decode into a `String` on PostgreSQL or MySQL.
- **#56, runtime driver switching.** `besu-any`'s `AnyDatabase`, chosen from the connection
  string; typed queries stay per-backend, which is the honest limit.
- **#61, table prefixes.** `Database::prefixed`. The prefix lives on `Sql`, because that is
  where every table name is written.
- **#53, streaming.** `Database::stream` / `Transaction::stream` decode row by row.
- **#67, savepoints.** `Transaction::transaction` and `savepoint(name, ..)`.
- **#58, JSON path operators.** `json_get` and `json_text` with a dotted path, rendered per
  dialect.
- **#54, rename detection.** `#[besu(renamed_from = "...")]` turns a drop plus an add into one
  `ALTER TABLE ... RENAME COLUMN`. Renaming a table still is not modelled.
- **#57, full-text search.** `matches(column, query)` and `Column::matches` bind the query and
  render `to_tsvector(..) @@ plainto_tsquery(..)` on PostgreSQL, `MATCH ... AGAINST` on MySQL
  and `MATCH` on SQLite. A `tsvector` column type, declaring the index or virtual table in
  `TableDef`, a language parameter, ranking and highlighting are still open.
- **#62, Studio UI.** `besu-studio` is a development-only server and single page: it serves
  the introspected schema, a paged row browser and a one-statement SQL endpoint. It binds
  loopback only unless `--allow-remote` is passed, because the query box runs arbitrary SQL
  and there is no authentication. See
  [The studio](architecture.md#the-studio).

## Contributing

Trait changes are the expensive part of this list: `Driver`, `Executor` and `Ddl` are public
API, and adding a method is a breaking change for every driver. Open an issue before starting
one of those. A new expression, a new `Change` variant or a CLI improvement is much easier to
land.
