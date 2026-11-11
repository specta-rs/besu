# Roadmap

Besu is early. This page lists what is missing, in rough priority order, and what each item
would actually require. The order is a judgement about impact versus cost, not a promise.

| # | Item | Why it matters | Rough size |
| --- | --- | --- | --- |
| 1 | [Eager loading / relations](#eager-loading--relations) | The biggest functional gap versus an ORM | Large |
| 2 | [Streaming results](#streaming-results) | Large result sets are fully buffered | Medium, breaking |
| 3 | [Rename detection in migrations](#rename-detection-in-migrations) | A rename currently looks like data loss | Medium |
| 4 | [Custom column types](#custom-column-types) | `DECIMAL`, enums and domain types are out of reach | Medium |
| 5 | [Runtime driver switching](#runtime-driver-switching) | Config-driven applications cannot pick a backend | Medium |
| 6 | [Full text search](#full-text-search) | No typed `MATCH` / `tsquery` | Medium |
| 7 | [JSON path operators](#json-path-operators) | JSON is stored but not queryable in a typed way | Medium |
| 8 | [u128 support](#u128-support) | Large identifiers have no lossless type | Small–medium |
| 9 | [wasm support](#wasm-support) | The builder could run in a browser; execution cannot | Large |
| 10 | [Table name prefixes](#table-name-prefixes) | Multi-tenant naming needs runtime prefixes | Small–medium |
| 11 | [Studio UI](#studio-ui) | No visual schema/data browser | Large |

## Eager loading / relations

Tracked as [#52](https://github.com/specta-rs/besu/issues/52).

Today a join returns a tuple: `(User, Option<Post>)`. There is no `User::posts`, no
`find_with_related`, and no way to load a graph without writing the join and grouping by hand.
That is the main reason to pick SeaORM over Besu for an object-shaped application.

What it would take:

- A relation descriptor on the table: a `HasMany<Child>` / `BelongsTo<Parent>` value carrying
  the local and foreign columns. `TableDef` currently has columns, indexes and foreign keys,
  but no relation list; this would either extend it or live in a separate registry.
- A `Decode` implementation for `Vec<T>`. Decoding is offset-based and has no notion of
  grouping, so nested results need either a second pass that groups rows by a key or a
  different row shape.
- A decision about SQL shape. A join plus grouping is one statement but duplicates the parent
  columns; a second query per relation is two statements and avoids the duplication but
  reintroduces the N+1 question. The project rule — the query you write is the SQL that runs
  — argues for batched, explicit second queries that are visible in the API.
- A `with(..)` method on `Select` that returns a nested row type, plus a way to spell the
  expected shape without a hand-written `Decode`.

## Streaming results

Tracked as [#53](https://github.com/specta-rs/besu/issues/53).

`Driver::query` returns `Vec<Row>`, and every builder collects before decoding. A million-row
scan is a million rows in memory.

What it would take:

- A streaming method on `Driver` (for example `query_stream` returning
  `impl Stream<Item = Result<Row, Error>> + Send`) and on `Executor`. sqlx already exposes
  `fetch`, so the drivers can implement it without new dependencies.
- A builder terminal such as `.stream()` that decodes row by row. `Decode` already works one
  row at a time; the missing piece is the stream type and the lifetime.
- A breaking change to the traits. This is the main reason it has not been done: a major
  version is the honest place for it.
- A policy for errors that occur mid-stream, after rows have already been handed to the
  caller.

`RawQuery` is buffered too, so it needs the same treatment.

## Rename detection in migrations

Tracked as [#54](https://github.com/specta-rs/besu/issues/54).

`diff` cannot tell a rename from a drop plus an add, so it deliberately refuses to guess. The
consequence is that renaming a column is classified as `Destructive` and a generated migration
drops the data.

What it would take:

- An explicit declaration. A pure diff can never be sure, so the API needs something like
  `#[besu(renamed_from = "old_name")]` on a field, or a rename map passed to `diff`.
- New `Change` variants (`RenameColumn`, `RenameTable`) and `Ddl` methods that render
  `ALTER TABLE ... RENAME COLUMN` / `RENAME TO`.
- A safety classification: a rename is `Risky` (it rewrites metadata and can break views), not
  `Destructive`, but it must never be inferred silently.
- CLI support so `besu generate` can consume the declaration from the snapshot.

## Custom column types

Tracked as [#55](https://github.com/specta-rs/besu/issues/55).

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
- Enum DDL: `Schema::enums` is stored but there is no `CREATE TYPE` step and the diff ignores
  enums. Custom types make this more urgent.

## Runtime driver switching

Tracked as [#56](https://github.com/specta-rs/besu/issues/56).

`Database<Dr>` is generic, so the backend is chosen at compile time. The CLI works around this
with `AnyDatabase`, a hand-written enum over the three drivers. Application code that reads a
URL from configuration cannot do the same without writing that enum itself.

What it would take:

- A type-erased driver. `Driver` is not object-safe: it has associated types (`Dialect`,
  `Row`, `Output`) and returns `impl Future`. Type erasure needs boxed rows, boxed futures and
  a dynamic `Value` path, which costs an allocation per row.
- Or an enum wrapper in the library, as the CLI has, generated for the enabled drivers. This
  keeps static typing per variant but makes every call site a `match`.
- A decision about `Decode`: with a dynamic driver, a model's `Decode<Dr>` impl cannot be
  selected statically, so rows would need a runtime decoder or a per-variant match.
- The same problem for migrations and DDL.

Until then, pick the driver at compile time or copy the CLI's enum pattern.

## Full text search

Tracked as [#57](https://github.com/specta-rs/besu/issues/57).

There is no typed `MATCH`, `tsquery` or `to_tsvector`. PostgreSQL users can write
`filter(raw("search_vector @@ to_tsquery('rust')"))`; SQLite users can query an FTS5 table with
`sql!`, but the schema layer does not create virtual tables.

What it would take:

- A `matches(column, query)` expression with per-dialect rendering: PostgreSQL
  `@@ to_tsquery`, MySQL `MATCH ... AGAINST`, SQLite `MATCH` against an FTS5 table.
- A `TypeDef` for `tsvector` and a way to declare an FTS index or virtual table in `TableDef`
  so `push` can create it.
- Language configuration (PostgreSQL's `regconfig`) as a parameter, not a hard-coded string.
- Ranking (`ts_rank`) and highlighting are separate functions.

## JSON path operators

Tracked as [#58](https://github.com/specta-rs/besu/issues/58).

`serde_json::Value` columns are stored and decoded, but there is no typed `->`, `->>`, `#>`,
`#>>` or `@>`. A query has to use `raw`, which loses the type check and cannot contain a
literal `?` because that is the placeholder marker.

What it would take:

- `ExprKind` variants for JSON access and containment, with dialect-specific rendering:
  PostgreSQL `jsonb -> 'key'`, SQLite `json_extract`, MySQL `JSON_EXTRACT` / `->>`.
- A path type (`&[&str]` or a small builder) and a result type: `->` returns JSON, `->>`
  returns text. That is two methods, not one, because the SQL types differ.
- A literal `?` escape in `sql!` for PostgreSQL's JSON existence operator, which needs the
  scanner to understand an escape sequence.
- Indexing support: a JSON path index is a DDL feature the schema IR cannot express.

## u128 support

Tracked as [#59](https://github.com/specta-rs/besu/issues/59).

`Value` has `Int(i64)`, `UInt(u64)` and no 128-bit variant; `TypeDef::UInt(u8)` describes a
width but the derive has no `u128` mapping, and `i128` is absent entirely.

What it would take:

- A `Value::UInt128` / `Value::Int128` (or a decimal string variant) and encode/decode impls
  in every driver.
- Dialect type names: PostgreSQL `NUMERIC(39,0)`, MySQL `DECIMAL(39,0)`, SQLite has no native
  type that holds it losslessly and would need text plus a range check.
- A `SqlType` and derive mapping for `u128` / `i128`.
- Care with `MAX_PARAMS` and bulk inserts is unaffected, but arithmetic on these values would
  still happen in the database, not in Rust.

## wasm support

Tracked as [#60](https://github.com/specta-rs/besu/issues/60).

The query builder is mostly `alloc`-only: `Dialect`, `Sql`, `Value`, `ExprKind` and the
builders do not need a filesystem or a socket. Execution is the problem: the drivers are
built on sqlx and tokio, which do not target `wasm32-unknown-unknown`.

What it would take:

- Make `besu-core` `no_std + alloc`. `Error` currently implements `std::error::Error`; that
  impl would need a `std` feature.
- Verify `chrono`, `uuid` and `serde_json` with the wasm target.
- A new driver for a browser storage API (IndexedDB, OPFS) or an HTTP bridge to a server. That
  driver has to supply `Decode` impls and a `Dialect` whose SQL a remote endpoint understands.
- A decision about blocking: browser storage is async but not a database connection pool; the
  `Driver` trait assumes a `Send` future and a `Send + Sync` driver, which may not fit.

Building and rendering SQL in the browser is achievable much sooner than executing it.

## Table name prefixes

Tracked as [#61](https://github.com/specta-rs/besu/issues/61).

Multi-tenant deployments often prefix every table (`tenant_42_users`). `TableDef.name` is a
`&'static str`, so a runtime prefix cannot be applied today, and the derive's
`#[besu(table = "...")]` is compile-time.

What it would take:

- A prefix carried by the database or the query builder, threaded into `push_table`, DDL and
  introspection. That changes `push_table` from a static name to a runtime-qualified one and
  makes some SQL text non-static.
- A matching change in introspection: it must strip the prefix before diffing, or the diff
  will see every table as new.
- A migration story: existing data has to be moved or the prefix applied at creation.
- An alternative already works: use a PostgreSQL schema (`#[besu(schema = "tenant_42")]`) or
  separate databases.

## Studio UI

Tracked as [#62](https://github.com/specta-rs/besu/issues/62).

There is no visual tool. Drizzle Studio is the model: browse tables, inspect rows, run a query
and see the SQL.

What it would take:

- A server (probably axum) exposing the schema IR (already serialisable), a paged row browser
  and a query endpoint.
- Authentication and authorisation, because a schema browser is a database browser.
- A frontend, or a small server-rendered UI.
- A safety model for writes: a Studio that can run arbitrary SQL defeats the point of the
  typed API unless it is explicitly a development tool.
- Packaging as `besu studio`, with the same URL dispatch the CLI already has.

This is deliberately last: it is useful, but it is not on the path to a dependable library.

## Smaller items

These are not large enough for their own section, but they are real. Each is a tracking issue.

- **[#63](https://github.com/specta-rs/besu/issues/63) — the right-hand side of a comparison
  is not checked.** `eq(users.id, posts.author_id)` checks `users` but not `posts`; column
  method syntax is always checked. This is the most valuable typesafety work left.
- **[#64](https://github.com/specta-rs/besu/issues/64) — SQLite cannot alter a column in
  place.** `push` refuses and `generate` warns; the table rebuild is unwritten.
- **[#65](https://github.com/specta-rs/besu/issues/65) — observability.** No tracing spans,
  query timing or slow-query hook. A `Logged<Dr>` wrapper would demonstrate the `Driver` seam.
- **[#66](https://github.com/specta-rs/besu/issues/66) — partial and expression indexes,
  check constraints, views.** `TableDef` models plain indexes and foreign keys.
- **[#67](https://github.com/specta-rs/besu/issues/67) — savepoints and nested
  transactions.** One level only.
- **[#68](https://github.com/specta-rs/besu/issues/68) — down migrations.** The runner is
  append-only by design.
- **[#69](https://github.com/specta-rs/besu/issues/69) — parameter limits for `IN` lists.**
  `MAX_PARAMS` is checked for bulk inserts but not elsewhere.
- **[#70](https://github.com/specta-rs/besu/issues/70) — batch insert chunking.** The error
  message names the batch size; splitting the work is yours.
- **[#71](https://github.com/specta-rs/besu/issues/71) — PostgreSQL composite `UNIQUE`.**
  Represented as an index, which `DROP INDEX` cannot remove.
- **[#72](https://github.com/specta-rs/besu/issues/72) — schema-qualified tables are diffed
  by name only.**
- **[#73](https://github.com/specta-rs/besu/issues/73) — `TimeTz`, enums and other native
  types** come back as `TypeDef::Custom`.
- **[#74](https://github.com/specta-rs/besu/issues/74) — a literal `?` escape** for `sql!`.
- **[#75](https://github.com/specta-rs/besu/issues/75) — publish-ready crate metadata.**
- **[#76](https://github.com/specta-rs/besu/issues/76) — update from model.**

## Contributing

Trait changes are the expensive part of this list: `Driver`, `Executor` and `Ddl` are public
API, and adding a method is a breaking change for every driver. Open an issue before starting
one of those. A new expression, a new `Change` variant or a CLI improvement is much easier to
land.
