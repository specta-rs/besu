# MySQL column types

MySQL has real unsigned integers, which makes it the only dialect where the whole `u64` range
is stored in a native integer. Everything wider than 64 bits becomes an exact decimal, and
timestamps are `DATETIME(6)` rather than `TIMESTAMP`.

## Type mapping

| Rust field | `SqlType` | `TypeDef` | SQL |
| --- | --- | --- | --- |
| `bool` | `Bool` | `Bool` | `BOOLEAN` |
| `i8` | `Int1` | `Int2` | `SMALLINT` |
| `i16` | `Int2` | `Int2` | `SMALLINT` |
| `i32` | `Int4` | `Int4` | `INT` |
| `i64` | `Int8` | `Int8` | `BIGINT` |
| `u8` | `UInt1` | `UInt(8)` | `TINYINT UNSIGNED` |
| `u16` | `UInt2` | `UInt(16)` | `SMALLINT UNSIGNED` |
| `u32` | `UInt4` | `UInt(32)` | `INT UNSIGNED` |
| `u64` | `UInt8` | `UInt(64)` | `BIGINT UNSIGNED` |
| `u128` | `U128` | `UInt(128)` | `DECIMAL(39,0)` |
| `i128` | `I128` | `Decimal(39, 0)` | `DECIMAL(39,0)` |
| `f32` | `Float4` | `Float4` | `FLOAT` |
| `f64` | `Float8` | `Float8` | `DOUBLE` |
| `String` | `Text` | `Text` | `TEXT` |
| `Vec<u8>` | `Bytes` | `Bytes` | `BLOB` |
| `uuid::Uuid` | `Uuid` | `Uuid` | `BINARY(16)` |
| `chrono::NaiveDate` | `Date` | `Date` | `DATE` |
| `chrono::NaiveTime` | `Time` | `Time` | `TIME` |
| `chrono::NaiveDateTime` | `Timestamp` | `Timestamp` | `DATETIME(6)` |
| `chrono::DateTime<Utc>` | `TimestampTz` | `TimestampTz` | `DATETIME(6)` |
| `serde_json::Value` | `Json` | `Json` | `JSON` |
| `#[besu(sql_type = "..")]` | `Char<N>`, `VarChar<N>`, `Decimal<P, S>` | matching | `CHAR(n)`, `VARCHAR(n)`, `DECIMAL(p,s)` |

Everything in the table round-trips through an insert and a `SELECT`, except a `Decimal`, which
has no decoder into its `String` yet. The `Custom` types below round-trip through DDL only.

## Integers

MySQL has signed and unsigned types at every width, so each marker maps onto the type of the
same name.

```rust
pub tiny: u8,    // TINYINT UNSIGNED
pub normal: u16, // SMALLINT UNSIGNED
pub big: u32,    // INT UNSIGNED
pub huge: u64,   // BIGINT UNSIGNED
```

A `u64` column holds the whole range. Decoding tries the unsigned type first and falls back to
the signed one, because a computed value such as `COUNT(*)` is reported as `BIGINT UNSIGNED`
even when the column behind it is signed. A signed column holding a negative number fails to
decode into `u64` rather than wrapping.

## Text

`String` is `TEXT`, which in MySQL is a 64 KiB text type with no length argument in the derive.
Use `sql_type` for a bounded column:

```rust
#[besu(sql_type = "besu::VarChar<64>")]
pub code: String,
```

A `TEXT` or `BLOB` column cannot be indexed without a prefix, and MySQL cannot index `JSON` at
all. That is why a `UNIQUE` on a `String` is a 255-character prefix key and a `UNIQUE` on a JSON
column renders as nothing.

## Binary

`Vec<u8>` is `BLOB` and round-trips unchanged.

```rust
pub payload: Vec<u8>,
```

## Time

`NaiveDateTime` and `DateTime<Utc>` are both `DATETIME(6)`. `DATETIME` is chosen over MySQL's
`TIMESTAMP` because `TIMESTAMP` is range-limited to 2038 and converts through the session time
zone on the way in and out.

```rust
pub seen: chrono::NaiveDateTime,                 // DATETIME(6)
pub recorded: chrono::DateTime<chrono::Utc>,     // DATETIME(6), no offset stored
```

There is no timezone-aware column: the offset is dropped, and the driver relies on the session
being pinned to UTC. `Sqlx::new` does that; `Sqlx::connect` does not.

Introspection reports a `DATETIME` as `Timestamp`. Both `Timestamp` and `TimestampTz` render
`DATETIME(6)`, and type normalisation folds the two, so a `DateTime<Utc>` column converges:
`push` is a no-op the second time. The offset is still not stored — it is the session's job,
and `Sqlx::new` pins the session to UTC.

## JSON

`serde_json::Value` is `JSON`. The driver serialises the document and binds it as text; MySQL
validates it, and a malformed document is an insert error rather than a stored string.

```rust
pub document: serde_json::Value,
```

A default on a JSON column has to be parenthesised, which the DDL generator does for you, and the
column cannot be indexed or made unique.

## UUID

`uuid::Uuid` is `BINARY(16)`, not a 36-character string: smaller, and ordered as bytes rather
than as text.

```rust
pub id: uuid::Uuid,
```

Introspection maps `binary(16)` to `Uuid`; a `varbinary(16)` of the same bytes, or a `blob`, is
reported as `Bytes`, so the declared length is what identifies the type.

## Decimal

`Decimal<PRECISION, SCALE>` renders `DECIMAL(p,s)`. Its Rust type is `String`, and no driver
decodes MySQL's `NEWDECIMAL` into a `String` yet, so use `sql!` with an explicit `CAST(... AS
CHAR)` or keep the column out of the model. See the [roadmap](../../roadmap.md).

## 128-bit integers

`u128` and `i128` are both `DECIMAL(39,0)`, which holds the whole range exactly. The driver
formats the digits straight into the wire buffer, and decoding parses an unscaled decimal back
into the 128-bit integer; a value with a fractional part is truncated, not rounded.

```rust
pub huge: u128,     // DECIMAL(39,0)
pub signed: i128,   // DECIMAL(39,0)
```

Introspection cannot tell the two apart, and the differ folds `UInt(128)` onto `Decimal(39,0)`,
so a declared `u128` converges against the `DECIMAL(39,0)` reported back.

## What introspection reports

| Native type | `TypeDef` |
| --- | --- |
| `tinyint(1)` | `Bool` (`UInt(8)` when `unsigned`) |
| `tinyint` / `smallint` / `mediumint` / `int` / `bigint` | `Int2`, `Int2`, `Int4`, `Int4`, `Int8` |
| any of those `UNSIGNED` | `UInt(8)`, `UInt(16)`, `UInt(32)`, `UInt(32)`, `UInt(64)` |
| `float` | `Float4` |
| `double`, `real` | `Float8` |
| `char(n)` | `Char(n)` |
| `varchar(n)` | `VarChar(n)`; with no length, `Text` |
| `tinytext`, `text`, `mediumtext`, `longtext` | `Text` |
| `binary(16)` | `Uuid` |
| other `binary`, `varbinary`, `*blob` | `Bytes` |
| `json` | `Json` |
| `date`, `time`, `datetime`, `timestamp` | `Date`, `Time`, `Timestamp`, `TimestampTz` |
| `decimal(p,s)`, `numeric(p,s)` | `Decimal(p, s)` |
| `year` | `Int2` |
| anything else | `Custom(column_type)` |

## Custom types

MySQL's inline `ENUM(...)` and `SET(...)` keep their native spelling, so a column arrives as
`Custom("enum('a','b')")`. They belong to the column rather than to the schema, so
`Schema::enums` is empty on MySQL and there is no derive spelling for one. Anything newer than
the dialect knows about — `GEOMETRY`, a `VECTOR` column — is `Custom` for the same reason. The
DDL round-trips; the value cannot be decoded into Rust.

Custom column types are on the [roadmap](../../roadmap.md); the rest of the gap against Drizzle is
in [the scoreboard](../../parity.md).

## Gotchas

- `BOOLEAN` is an alias for `TINYINT(1)`. A `tinyint(1)` column introspects as `Bool`, except
  `tinyint(1) unsigned`, which is a `u8` and introspects as `UInt(8)`. A `tinyint` of another
  width is `Int2`.
- A `UNIQUE` on a `String` is a prefix key over 255 characters. Two rows that agree in the first
  255 characters collide even when the full values differ.
- A `UNIQUE` on a `JSON` column renders nothing, so the schema reports drift on every push.
- `TIMESTAMP` columns introspect as `TimestampTz`, so a snapshot of one renders `DATETIME(6)`
  when it is used to create a fresh table. Keep the choice of column type in a migration rather
  than round-tripping it.
- `DECIMAL` cannot be read back into the `String` that `Decimal` names yet; `sql!` with a cast
  is the way through.
- A `BLOB` or `TEXT` index needs a prefix length, and `CREATE INDEX` cannot know the column's
  type, so the DDL generator writes indexes inside `CREATE TABLE`.
