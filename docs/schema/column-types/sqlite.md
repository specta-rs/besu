# SQLite column types

SQLite has five storage classes and no column types beyond them. What you declare is an
affinity hint, so the mapping is about agreeing on a spelling that introspection and the
driver both understand, not about storage width.

## Type mapping

| Rust field | `SqlType` | `TypeDef` | SQL |
| --- | --- | --- | --- |
| `bool` | `Bool` | `Bool` | `BOOLEAN` |
| `i8` | `Int1` | `Int2` | `INTEGER` |
| `i16` | `Int2` | `Int2` | `INTEGER` |
| `i32` | `Int4` | `Int4` | `INTEGER` |
| `i64` | `Int8` | `Int8` | `INTEGER` |
| `u8` | `UInt1` | `UInt(8)` | `INTEGER` |
| `u16` | `UInt2` | `UInt(16)` | `INTEGER` |
| `u32` | `UInt4` | `UInt(32)` | `INTEGER` |
| `u64` | `UInt8` | `UInt(64)` | `INTEGER` |
| `u128` | `U128` | `UInt(128)` | `TEXT` |
| `i128` | `I128` | `Decimal(39, 0)` | `TEXT` |
| `f32` | `Float4` | `Float4` | `REAL` |
| `f64` | `Float8` | `Float8` | `REAL` |
| `String` | `Text` | `Text` | `TEXT` |
| `Vec<u8>` | `Bytes` | `Bytes` | `BLOB` |
| `uuid::Uuid` | `Uuid` | `Uuid` | `UUID` |
| `chrono::NaiveDate` | `Date` | `Date` | `DATE` |
| `chrono::NaiveTime` | `Time` | `Time` | `TIME` |
| `chrono::NaiveDateTime` | `Timestamp` | `Timestamp` | `TIMESTAMP` |
| `chrono::DateTime<Utc>` | `TimestampTz` | `TimestampTz` | `TIMESTAMP` |
| `serde_json::Value` | `Json` | `Json` | `JSON` |
| `besu::SqlInterval` | the type itself | `Interval` | `TEXT` |
| `besu::Inet` | the type itself | `Inet` | `TEXT` |
| `besu::Cidr` | the type itself | `Cidr` | `TEXT` |
| `#[besu(sql_type = "besu::Bit<n>")]` | `Bit<n>` | `Bit(n)` | `TEXT` |
| `#[besu(sql_type = "besu::VarBit<n>")]` | `VarBit<n>` | `VarBit(n)` | `TEXT` |
| `besu::Range<T>` | `Range<T>` | `Range(T)` | `TEXT` |
| `besu::Geometry` | the type itself | `Geometry { .. }` | `BLOB` |
| `#[besu(sql_type = "besu::Point<srid>")]` | the shape markers | `Geometry { kind, srid }` | `BLOB` |
| `besu::Vector` | the type itself | `Vector(_)` | `TEXT` |
| `#[besu(sql_type = "..")]` | e.g. `VarChar<64>` | `VarChar(64)` | `TEXT` |

Every type in the table round-trips through an insert and a `SELECT`. The only types that do not
are the `Custom` ones below, which exist in a database but not in Rust.

## Integers

Every signed and unsigned width below 128 bits is `INTEGER`, which is a signed 64-bit value.
The Rust type is still what the struct says, so the driver range-checks on the way in and out.

```rust
pub tiny: u8,      // INTEGER
pub big: u64,      // INTEGER
```

A `u64` above `i64::MAX` cannot be stored. The dialect rejects it when the value is bound,
with an error that points at `u128` as the alternative; it does not wrap or truncate. `i8` is
widened to a 16-bit marker because no database has an 8-bit signed integer, which changes
nothing on SQLite where both are `INTEGER`.

## Text

`String` is `TEXT`, and so is anything declared with `Char<N>`, `VarChar<N>` or `Decimal<P, S>`.
SQLite has no length or precision limit, so the size exists only in the IR.

```sql
"label" TEXT NOT NULL,      -- besu::VarChar<64>
"amount" TEXT NOT NULL,     -- besu::Decimal<10, 2>
```

## Binary

`Vec<u8>` is `BLOB` and round-trips byte for byte.

```rust
pub payload: Vec<u8>,
```

## Time

Dates and times are stored as text in the formats both drivers use: `%F` for a date, `%T%.f`
for a time, `%F %T%.f` for a timestamp, and RFC 3339 (with an offset) for a `DateTime<Utc>`.

```rust
pub on: chrono::NaiveDate,                       // "2024-05-17"
pub at: chrono::NaiveTime,                       // "13:45:06"
pub seen: chrono::NaiveDateTime,                 // "2024-05-17 13:45:06"
pub recorded: chrono::DateTime<chrono::Utc>,     // "2024-05-17T13:45:06+00:00"
```

The declared names `DATE`, `TIME` and `TIMESTAMP` are names, not storage classes: the value is
text. Ordering is therefore lexicographic, which is correct for these ISO-8601 shapes and wrong
for anything else you write into the column by hand.

## JSON

`serde_json::Value` is `JSON` and is stored as UTF-8 text. The driver binds the serialised
document and parses it back on decode, so a JSON column round-trips as a value, not a string.

```rust
pub document: serde_json::Value,
```

`Jsonb` has no derive spelling — write `#[besu(sql_type = "besu::Jsonb")]` if you need the
marker, and it renders `JSON` here anyway.

## UUID

`uuid::Uuid` is declared `UUID` and stored as a 16-byte `BLOB`, not as the 36-character string.
Both drivers write and read it the same way.

```rust
pub id: uuid::Uuid,
```

Because the stored value is binary, a hand-written `WHERE id = '...'` comparison does not match.
Bind a `Uuid` and the comparison is a blob comparison.

## Decimal and 128-bit integers

There is no decimal type and no integer wider than 64 bits. A decimal column is `TEXT`, because
`NUMERIC` affinity would convert the stored digits to a float and lose the exact value that
makes a decimal worth having. `u128` and `i128` are `TEXT` holding the decimal digits.

```sql
"amount" TEXT NOT NULL,     -- Decimal(10, 2), exact as written
"huge" TEXT NOT NULL        -- u128, up to 39 digits
```

The driver parses the digits back into `u128`/`i128`, so the full range round-trips. Comparisons
and `ORDER BY` on such a column are lexicographic unless you cast it to a number in SQL.

`Decimal<PRECISION, SCALE>` is the same `TEXT` storage, bound as the exact decimal text, so its
`String` round-trips unchanged and `amount.eq("1.23")` works.

## Interval, addresses and bit strings

SQLite has no interval, network or bit type, so each is stored as its canonical text in a `TEXT`
column:

```rust
use besu::{BitString, Cidr, Inet, SqlInterval};

pub span: SqlInterval,    // "1 days 02:00:00"
pub host: Inet,           // "192.168.0.1/24"
pub block: Cidr,          // "192.168.1.0/24"
#[besu(sql_type = "besu::Bit<5>")]
pub flags: BitString,     // "10110"
```

The driver binds the same spelling `Display` writes and parses it on the way back, so a value
round-trips exactly. There is no ordering or arithmetic a database could do with them: `ORDER BY`
is text order, and a comparison against a hand-written literal is a string comparison. SQLite
collapses all three onto `TEXT` when it introspects them, so the declaration converges on a second
push.

## Ranges, spatial values and vectors

SQLite has no range, spatial or vector type, so all three are stored in their canonical form:

```rust
use besu::{Geometry, Range, Vector};

pub window: Range<i32>,       // "[1,5)" in a TEXT column
pub day: Range<chrono::NaiveDate>,
pub point: Geometry,          // EWKB bytes in a BLOB column
#[besu(sql_type = "besu::Point<4326>")]
pub located: Geometry,
pub embedding: Vector,        // "[1,2,3]" in a TEXT column
#[besu(sql_type = "besu::VectorOf<3>")]
pub xyz: Vector,
```

A range is bound as its `[a,b)` spelling and parsed back on decode, so there is no range
semantics: `[1,5]` stays `[1,5]`, and comparison and `ORDER BY` are text order. A spatial value
is the EWKB bytes exactly as PostGIS sends them, in a `BLOB`; `Geometry` decodes from that blob
and keeps its SRID and shape. A vector is its `[1,2,3]` text.

The declarations fold onto the storage: `Range` and `Vector` to `Text`, `Geometry` and
`Geography` to `Bytes`, so a second push is a no-op.

SpatiaLite adds a real `geometry` type, but the declaration here is a plain `BLOB` and Besu does
not detect SpatiaLite columns. A SpatiaLite geometry column read by introspection is
`Custom("geometry")`, not `Geometry`; keep such a column out of the declaration (or declare it
`Custom`) and read it through `sql!`.

## Arrays

SQLite has no array type. A `Vec<T>` field renders `TEXT` and binding an array is an error before
the statement is sent, rather than being silently stored as a string. Store a collection as
`JSON` and query it with the JSON helpers.

## What introspection reports

SQLite's catalogue reports the declared type name, which is mapped back as follows:

| Declared | `TypeDef` |
| --- | --- |
| `INTEGER` (or anything containing `INT`) | `Int8` |
| `REAL` (or `FLOA`, `DOUB`) | `Float8` |
| `TEXT`, `CLOB` (or empty) | `Text` |
| `BLOB` | `Bytes` |
| `BOOLEAN`, `UUID`, `JSON`, `DATE`, `TIME`, `TIMESTAMP`, `DATETIME` | the matching type |
| `NUMERIC`, `DECIMAL` | `Decimal(0, 0)` |
| anything else | `Custom(name)` |

The differ folds the types SQLite cannot distinguish — every integer width to `Int8`, `UInt(128)`
to `Text`, `Char`/`VarChar`/`Decimal` to `Text`, `Jsonb` to `Json`, `TimestampTz` to `Timestamp`,
a fractional-second precision to the plain time type,
`Bit`/`VarBit`/`Interval`/`Inet`/`Cidr`/`Set`/`Range`/`Vector` to `Text`, `Geometry`/`Geography`
to `Bytes`, and an array to `Json` — so a declared `i32` converges against the `INTEGER` that
introspection reports. Without that folding, every push would report a column change that changes
nothing. A PostgreSQL domain folds onto its base type here.

## Custom types

A `Custom` type only arrives from introspection of a column declared with a name Besu does not
know — an existing `MONEY` or `GEOMETRY` column, for example. The type name is kept verbatim so
generated DDL round-trips, but no driver decodes it: there is no Rust type to decode into. Keep
such a column out of your model, or read it through `sql!` and a cast.

Intervals, network addresses, bit strings, MySQL's `SET`, ranges, vectors and spatial values
have their own `TypeDef` variants and are not `Custom`: on SQLite they are stored as `TEXT` or
`BLOB` and round-trip through the driver. A SpatiaLite `geometry` column is the exception,
described above; Besu ships no `Decode` implementation for a `Custom` type.

## Gotchas

- A `u64` above `i64::MAX` is an encode error, not a wrapped value. Use `u128` for a number that
  large.
- Ordering on a `u128`, `i128` or `Decimal` column is text ordering. Cast in SQL when the order
  matters.
- `DATE`, `TIME`, `TIMESTAMP` and `UUID` are declared names, not storage classes: the values are
  bound as text (or 16 bytes for a UUID), so a comparison against a hand-written literal is a
  text or blob comparison.
- Widening `i32` to `i64` is invisible to `push`, because both are `INTEGER`. The change is real
  in Rust and absent in the database.
- A `BLOB` column bound from a `String` writes text into it; SQLite does not convert. Bind the
  type the column was declared with.
- A `Range` or `Geometry` declared here is not a database type: another client sees `TEXT` or
  `BLOB`, and SQL that compares it has to use the text or blob form.
- SpatiaLite's own geometry columns are `Custom("geometry")`, not `Geometry`; SpatiaLite is not
  detected.
