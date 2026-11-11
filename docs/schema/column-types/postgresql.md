# PostgreSQL column types

PostgreSQL has a native type for every value Besu models, with one exception: it has no
unsigned integers. The mapping therefore widens an unsigned column to the smallest signed type
that holds it, or to `NUMERIC` when no integer is wide enough.

## Type mapping

| Rust field | `SqlType` | `TypeDef` | SQL |
| --- | --- | --- | --- |
| `bool` | `Bool` | `Bool` | `BOOLEAN` |
| `i8` | `Int1` | `Int2` | `SMALLINT` |
| `i16` | `Int2` | `Int2` | `SMALLINT` |
| `i32` | `Int4` | `Int4` | `INTEGER` |
| `i64` | `Int8` | `Int8` | `BIGINT` |
| `u8` | `UInt1` | `UInt(8)` | `SMALLINT` |
| `u16` | `UInt2` | `UInt(16)` | `INTEGER` |
| `u32` | `UInt4` | `UInt(32)` | `BIGINT` |
| `u64` | `UInt8` | `UInt(64)` | `NUMERIC(20,0)` |
| `u128` | `U128` | `UInt(128)` | `NUMERIC(39,0)` |
| `i128` | `I128` | `Decimal(39, 0)` | `NUMERIC(39,0)` |
| `f32` | `Float4` | `Float4` | `REAL` |
| `f64` | `Float8` | `Float8` | `DOUBLE PRECISION` |
| `String` | `Text` | `Text` | `TEXT` |
| `Vec<u8>` | `Bytes` | `Bytes` | `BYTEA` |
| `uuid::Uuid` | `Uuid` | `Uuid` | `UUID` |
| `chrono::NaiveDate` | `Date` | `Date` | `DATE` |
| `chrono::NaiveTime` | `Time` | `Time` | `TIME` |
| `chrono::NaiveDateTime` | `Timestamp` | `Timestamp` | `TIMESTAMP` |
| `chrono::DateTime<Utc>` | `TimestampTz` | `TimestampTz` | `TIMESTAMPTZ` |
| `serde_json::Value` | `Json` | `Json` | `JSON` |
| `#[besu(sql_type = "..")]` | `Char<N>`, `VarChar<N>`, `Decimal<P, S>`, `Jsonb` | matching | `CHAR(n)`, `VARCHAR(n)`, `NUMERIC(p,s)`, `JSONB` |

Everything in the table round-trips through an insert and a `SELECT`, except a `Decimal`, which
has no decoder into its `String` yet. The `Custom` types below round-trip through DDL only.

## Integers

There are no unsigned types, so each unsigned marker is widened by one step and the range is
enforced in Rust.

```rust
pub tiny: u8,   // SMALLINT
pub normal: u16, // INTEGER
pub big: u32,   // BIGINT
pub huge: u64,  // NUMERIC(20,0)
```

A `u64` is always bound as `NUMERIC`, even when the value fits in an `i64`, so one SQL type
covers every value the column can hold. A negative value inserted by hand into a `u64` column is
accepted by the server and rejected when it is decoded back into Rust.

## Text

`String` is `TEXT`. `Char<N>` and `VarChar<N>` are reachable through `sql_type` and render
`CHAR(n)` and `VARCHAR(n)`.

```rust
#[besu(sql_type = "besu::VarChar<64>")]
pub code: String,
```

A `VARCHAR` declared with no length is reported by introspection as `TEXT`, because that is what
it is.

## Binary

`Vec<u8>` is `BYTEA` and round-trips unchanged.

```rust
pub payload: Vec<u8>,
```

## Time

`NaiveDateTime` is `TIMESTAMP` and `DateTime<Utc>` is `TIMESTAMPTZ`. The driver encodes UTC and
decodes into UTC; `DateTime<Local>` has no `Decode` impl, so it does not compile.

```rust
pub seen: chrono::NaiveDateTime,                 // TIMESTAMP
pub recorded: chrono::DateTime<chrono::Utc>,     // TIMESTAMPTZ
```

`TIMETZ` is modelled as `TypeDef::TimeTz` so introspection and DDL round-trip, but no Rust field
maps to it and no driver decodes it. sqlx represents it with its own `PgTimeTz` struct, which has
no `Value` variant. Use `Time` or `TimestampTz` in code that reads rows.

## JSON

`serde_json::Value` is `JSON`. The `Jsonb` marker is available through `sql_type` and renders
`JSONB`; both decode into a `serde_json::Value`.

```rust
#[besu(sql_type = "besu::Jsonb")]
pub document: serde_json::Value,
```

The driver validates the document once and binds the borrowed bytes as the driver's JSON type,
so a malformed document fails before the statement is sent.

## UUID

`uuid::Uuid` is PostgreSQL's native `UUID` and is sent as 16 bytes on the wire.

```rust
pub id: uuid::Uuid,
```

## Decimal

`Decimal<PRECISION, SCALE>` renders `NUMERIC(p,s)`. Its Rust type is `String`, and that is where
the gap is: no driver decodes a `NUMERIC` into a `String` yet, so a decimal column can be
declared and diffed but not read into the model. Use `sql!` with an explicit `::text` cast, or
keep the column out of the model until the decoder lands on the [roadmap](../../roadmap.md).

## 128-bit integers

`u128` and `i128` both become `NUMERIC(39,0)`, which is exact and wide enough for `u128::MAX`.
The driver writes the value straight into PostgreSQL's binary `NUMERIC` format, so the whole
range round-trips without a decimal library.

```rust
pub huge: u128,     // NUMERIC(39,0)
pub signed: i128,   // NUMERIC(39,0)
```

The two share a spelling, so introspection cannot tell them apart: a `NUMERIC(39,0)` column is
reported as `UInt(128)`. The DDL is identical either way, which is why a push still converges.

## What introspection reports

| Native type | `TypeDef` |
| --- | --- |
| `smallint`, `integer`, `bigint` | `Int2`, `Int4`, `Int8` |
| `numeric(20,0)` | `UInt(64)` |
| `numeric(39,0)` | `UInt(128)` |
| any other `numeric` / `decimal` | `Decimal(precision, scale)` |
| `character varying(n)` | `VarChar(n)`; with no length, `Text` |
| `character(n)` | `Char(n)` |
| `time with time zone` | `TimeTz` |
| `USER-DEFINED`, `ARRAY` | `Custom(udt_name)` |
| anything else | `Custom(data_type)` |

The differ folds the unsigned markers onto their signed storage — `UInt(8)` to `Int2`, `UInt(16)`
to `Int4`, `UInt(32)` to `Int8`, `UInt(64)` to `Decimal(20,0)`, `UInt(128)` to `Decimal(39,0)` —
so a declared `u16` converges against the `INTEGER` the database reports.

## Custom types

Enums, domains, composite types, arrays, ranges, `INET`/`CIDR`, `INTERVAL`, `BIT`, `tsvector`
and extension types such as `pgvector` all come back as `Custom(name)`. The name is preserved, so
generated DDL round-trips, but there is no Rust type to decode into and the derive has no
spelling for one. Keep such a column out of the model and read it through `sql!`.

A column over a PostgreSQL enum is `Custom("mood")`; the type itself lives in `Schema::enums`,
which is introspected, diffed and rendered as `CREATE TYPE`. The derive cannot declare an enum,
so the type list belongs to a snapshot (`besu pull`) or a migration.

## Gotchas

- `NUMERIC(20,0)` and `NUMERIC(39,0)` are recognised by precision and scale, so a hand-written
  decimal of that shape is read back as an unsigned integer.
- A `CHAR(n)` value is blank-padded by the server, so `char(2)` holding `'a'` reads back as
  `'a '`.
- `DateTime<Local>` is declared `TIMESTAMPTZ` by name matching and then fails to compile when a
  query returns the model. Use `DateTime<Utc>`.
- `TIMETZ` can be created, introspected and rendered, but not read into a Rust value.
- An enum, domain or array column round-trips through `push`/`pull` and is invisible to the
  derive. Dropping one is destructive and is refused without `push_destructive`.
- A `NUMERIC` with no precision, such as a bare `numeric` column, introspects as
  `Custom("numeric")` rather than a decimal.
