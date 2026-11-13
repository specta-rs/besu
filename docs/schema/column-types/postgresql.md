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
| `Vec<T>` | `Array<T>` | `Array(T)` | `<T>[]` |
| `besu::Range<T>` | `Range<T>` | `Range(T)` | `int4range`, `int8range`, `numrange`, `tsrange`, `tstzrange`, `daterange` |
| `besu::Geometry` | the type itself | `Geometry { .. }` | `GEOMETRY` |
| `#[besu(sql_type = "besu::Point<srid>")]` | `Point<srid>` and the other shapes | `Geometry { kind, srid }` | `GEOMETRY(POINT,4326)` |
| `#[besu(sql_type = "besu::GeographyPoint<srid>")]` | the `Geography*` markers | `Geography { kind, srid }` | `GEOGRAPHY(POINT,4326)` |
| `besu::Vector` | the type itself | `Vector(0)` | `VECTOR` |
| `#[besu(sql_type = "besu::VectorOf<n>")]` | `VectorOf<n>` | `Vector(n)` | `VECTOR(n)` |
| `besu::SqlInterval` | the type itself | `Interval` | `INTERVAL` |
| `besu::Inet` | the type itself | `Inet` | `INET` |
| `besu::Cidr` | the type itself | `Cidr` | `CIDR` |
| `#[besu(sql_type = "besu::Bit<n>")]` | `Bit<n>` | `Bit(n)` | `BIT(n)` |
| `#[besu(sql_type = "besu::VarBit<n>")]` | `VarBit<n>` | `VarBit(n)` | `BIT VARYING(n)` |
| `#[derive(SqlEnum)]` | the enum itself | `Enum` | the enum's name |
| `#[besu(sql_type = "..")]` | `Char<N>`, `VarChar<N>`, `Decimal<P, S>`, `Jsonb` | matching | `CHAR(n)`, `VARCHAR(n)`, `NUMERIC(p,s)`, `JSONB` |

Everything in the table round-trips through an insert and a `SELECT`. A pulled `Custom` type
round-trips through DDL only. The rows for ranges, spatial columns and vectors describe native
PostgreSQL and extension types: a range needs nothing extra, while PostGIS and `pgvector` must
be installed (`CREATE EXTENSION postgis` / `CREATE EXTENSION vector`).

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

A precision in the declaration is part of the type: `TIME(3)` is `TimeP(3)` and a bare `TIME` is
`Time`. `NaiveTime` is declared `TIME` and keeps microseconds.

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

`Decimal<PRECISION, SCALE>` renders `NUMERIC(p,s)` and its Rust type is `String` holding the
exact text. The value is written in PostgreSQL's binary `NUMERIC` format — a text parameter
would not be accepted, because no `text` to `numeric` cast exists — and read back with the
column's own display scale, so `1.90` stays `1.90`.

```rust
#[besu(sql_type = "besu::Decimal<10,2>")]
pub amount: String,
```

A decimal is compared with the decimal text it was declared with: `amounts.amount.eq("1.23")`
compiles because `IntoSqlValue<Decimal<P, S>>` is implemented for `String`, `&str` and `Cow`,
for every precision and scale.

`sum` and `avg` cast to `DOUBLE PRECISION`, which loses the exactness that made the column a
decimal. `sum_as`/`avg_as` — or the `SumAs` and `AvgAs` methods on a column — leave the
database's `NUMERIC` result alone:

```rust
db.select(amounts)
    .pick(amounts.amount.sum_as::<Decimal<20, 2>>())
    .all()
    .await?;
```

## 128-bit integers

`u128` and `i128` both become `NUMERIC(39,0)`, which is exact and wide enough for `u128::MAX`.
The driver writes the value straight into PostgreSQL's binary `NUMERIC` format, so the whole
range round-trips without a decimal library.

```rust
pub huge: u128,     // NUMERIC(39,0)
pub signed: i128,   // NUMERIC(39,0)
```

The two share a spelling, so introspection cannot tell them apart: a `NUMERIC(39,0)` column is
reported as `Decimal(39, 0)`, and the differ folds `UInt(128)` onto the same type, which is why
a push still converges.

## Enums

A Rust enum becomes a named PostgreSQL type. The name defaults to the enum's `snake_case` name and
the labels are the variants:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, besu::SqlEnum)]
#[besu(rename_all = "snake_case")]
pub enum Mood {
    Happy,
    #[besu(rename = "meh")]
    Neutral,
    Sad,
}

#[derive(Table)]
pub struct Review {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    #[besu(sql_type = "Mood")]
    pub mood: Mood,
}
```

`push` renders `CREATE TYPE "mood" AS ENUM ('happy', 'meh', 'sad')` before the table, and the
column is declared `"mood"`. A label appended to the Rust enum diffs as
`ALTER TYPE "mood" ADD VALUE '...'`; a label that disappears is destructive and is refused, because
PostgreSQL has no `ALTER TYPE ... DROP VALUE`.

The server reports the column as `Custom("mood")`, and `TypeDef::Enum` folds onto that, so a
declaration and the database converge. A label is bound as an untyped parameter, which lets the
server infer the enum from the column — a `TEXT` parameter would not be assignable to an enum
without a cast.

An enum with no name (`#[besu(name = "")]`) has nothing to create, so it is stored as `TEXT`. That
is how a schema shared with MySQL keeps working; see the [declaration guide](../declaration.md#enums).

## What introspection reports

| Native type | `TypeDef` |
| --- | --- |
| `smallint`, `integer`, `bigint` | `Int2`, `Int4`, `Int8` |
| any `numeric` / `decimal` | `Decimal(precision, scale)`; a bare `numeric` is `Custom("numeric")` |
| `character varying(n)` | `VarChar(n)`; with no length, `Text` |
| `character(n)` | `Char(n)` |
| `time[(p)] [with|without] time zone` | `Time`, `TimeP(p)`, `TimeTz`, `TimeTzP(p)` |
| `timestamp[(p)] [with|without] time zone` | `Timestamp`, `TimestampP(p)`, `TimestampTz`, `TimestampTzP(p)` |
| `bit(n)`, `bit varying(n)` | `Bit(n)`, `VarBit(n)` |
| `interval` | `Interval` |
| `inet` | `Inet` |
| `cidr` | `Cidr` |
| `int4range`, `int8range`, `numrange`, `tsrange`, `tstzrange`, `daterange` | `Range(Int4)`, `Range(Int8)`, `Range(Decimal(0,0))`, `Range(Timestamp)`, `Range(TimestampTz)`, `Range(Date)` |
| `geometry[(shape[, srid])]`, `geography[(shape[, srid])]` | `Geometry { kind, srid }`, `Geography { kind, srid }` |
| `vector[(n)]` | `Vector(n)`; a bare `vector` is `Vector(0)` |
| `<inner>[]` | `Array(inner)`, read through `pg_type.typelem`/`typarray` |
| a domain | `Domain(name, base)`, where `base` is what the domain is built on |
| an enum | `Custom(name)`, and listed in `Schema::enums` |
| a column with `COLLATE` | `Collated(inner, name)` |
| anything else | `Custom(format_type(oid, atttypmod))` |

Type modifiers are kept: `timestamp(3)` comes back as `TimestampP(3)` and `bit(5)` as `Bit(5)`,
so a `pull` followed by a `push` does not silently round the precision. A domain keeps its own
name *and* its base type, so the column is never altered to the base type and a value is
decoded through it. A column collation is kept when it differs from the type's default.

The differ folds the unsigned markers onto their signed storage — `UInt(8)` to `Int2`, `UInt(16)`
to `Int4`, `UInt(32)` to `Int8`, `UInt(64)` to `Decimal(20,0)`, `UInt(128)` to `Decimal(39,0)` —
so a declared `u16` converges against the `INTEGER` the database reports. The fold reaches inside
an array type, so `Array(UInt(16))` converges against `Array(Int4)`.

## Interval

`SqlInterval` is PostgreSQL's native `INTERVAL`, with the three fields the type really has:
months, days and microseconds. They stay separate because a month is not thirty days and a day
is not always 86,400 seconds.

```rust
use besu::SqlInterval;

pub trial: SqlInterval,   // INTERVAL, e.g. "14 mons 3 days 01:30:00"
```

```rust
let span = SqlInterval::new(0, 1, 90 * 60 * 1_000_000); // one day and ninety minutes
let span = SqlInterval::from(chrono::Duration::hours(36));
```

`Display` writes PostgreSQL's canonical spelling — `26 mons 3 days 01:30:00` — and `FromStr`
accepts it plus the unit names (`1 year 2 mons 3 days 04:05:06`). The value is written in
PostgreSQL's binary interval format (microseconds, days, months) and read back with all three
fields intact.

The name is `SqlInterval` rather than `Interval` because `besu::Interval` is the interval
*literal* builder used by `date_add` and friends.

## Network addresses

`Inet` is an address with an optional prefix and `Cidr` is a network; both keep PostgreSQL's
canonical text.

```rust
use besu::{Cidr, Inet};

pub peer: Inet,         // 192.168.0.1/24, or 192.168.0.1 for a full prefix
pub block: Cidr,        // 192.168.1.0/24, always with a prefix
```

`Cidr::new("192.168.1.7/24")` clears the host bits, so the value is the network
`192.168.1.0/24`. `Inet` writes a `/32` or `/128` prefix the way PostgreSQL does — by leaving it
out. Both are encoded in the binary network format (family, netmask, `is_cidr`, address length,
address) and both decode from the binary bytes, so IPv6 works too.

## Bit strings

`BitString` is a string of `0` and `1`, and the width is part of the column's type:

```rust
use besu::BitString;

#[besu(sql_type = "besu::Bit<8>")]
pub flags: BitString,       // BIT(8), exactly eight bits
#[besu(sql_type = "besu::VarBit<0>")]
pub tail: BitString,        // BIT VARYING, unbounded
```

A `BIT(n)` value must be exactly `n` bits; a `VARBIT(n)` value at most `n`. The value is written
in PostgreSQL's binary bit format (bit length, then the bytes most significant bit first), so
`BitString::new("10110")` round-trips as those five bits.

## Arrays

A `Vec<T>` field is `Array<T>`, declared as `<T>[]` and written with PostgreSQL's binary array
format. The element's declared type travels with the value because the array header names the
element OID and the server rejects a mismatch.

```rust
pub tags: Vec<String>,     // TEXT[]
pub scores: Vec<i32>,      // INTEGER[]
```

The operators and functions are in the [operator reference](../../queries/operators.md#arrays):
containment, overlap, concatenation, subscripts, `array_length`, `array_agg` and the rest.
`in_list` binds one array parameter rather than one per value.

A `Vec` decodes one dimension. A multi-dimensional array, or one containing a `NULL` element,
needs `sql!`. `Vec<String>` binds its elements as `TEXT[]`, so a `VARCHAR(n)[]` column needs the
typed expression path rather than a whole-model insert.

## Ranges

A `Range<T>` field is one of PostgreSQL's six built-in range types, named after its subtype:

```rust
use besu::Range;

pub ids: Range<i32>,                          // int4range
pub huge: Range<i64>,                         // int8range
pub window: Range<chrono::NaiveDateTime>,     // tsrange
pub window_tz: Range<chrono::DateTime<chrono::Utc>>, // tstzrange
pub day: Range<chrono::NaiveDate>,            // daterange
#[besu(sql_type = "besu::Range<besu::Decimal<0, 0>>")]
pub amount: Range<String>,                    // numrange
```

`numrange`'s subtype is an unconstrained `numeric`, so the precision and scale on the marker are
not part of the column type: `Range<Decimal<10,2>>` still declares `numrange`, and introspection
reports `Range(Decimal(0, 0))`. A range over any other subtype has no built-in PostgreSQL type
and is stored as `TEXT`.

The value is [`besu::Range`](https://docs.rs/besu/latest/besu/enum?search=Range), which keeps
each bound and whether it is included:

```rust
let ids = Range::closed(1, 5);                    // [1,5]
let window = Range::new(Bound::Included(1), Bound::Excluded(5)); // [1,5)
let nothing = Range::empty();
assert!(window.contains(&2));
```

`Display` writes PostgreSQL's `[a,b)` spelling and `FromStr` reads it, including `empty`, an
open end and a quoted bound. The value is written in PostgreSQL's binary range format — a flag
byte, then each finite bound as the subtype's own encoding behind a four-byte length — and read
back the same way, so `int8range` keeps values wider than 64 bits and `tstzrange` keeps the
instant.

PostgreSQL canonicalises a **discrete** range: `[1,5]` is stored as `[1,6)`, and `(1,5)` as
`[2,5)`. `int4range`, `int8range` and `daterange` are discrete; the timestamp and numeric ranges
are continuous and keep their bounds exactly. A model compares against what the server returns,
so write the canonical spelling when it matters — or use `closed_open`.

An array of ranges (`Vec<Range<i32>>`) is a real `int4range[]`: the binary array header names the
range's OID, which the driver knows for all six built-in types.

## Spatial

PostGIS columns are `Geometry` values carrying EWKB:

```rust
use besu::Geometry;

pub any: Geometry,                                      // GEOMETRY
#[besu(sql_type = "besu::Point<4326>")]
pub home: Geometry,                                     // GEOMETRY(POINT,4326)
#[besu(sql_type = "besu::LineString<4326>")]
pub route: Geometry,                                    // GEOMETRY(LINESTRING,4326)
#[besu(sql_type = "besu::GeographyPoint<4326>")]
pub area: Geometry,                                     // GEOGRAPHY(POINT,4326)
```

The shape and SRID are part of the declared column type: PostgreSQL reports them through
`format_type` as `geometry(Point,4326)`, so they are carried in
`TypeDef::Geometry { kind, srid }` and `TypeDef::Geography { kind, srid }`. A bare `Geometry`
field is the unmodified `GEOMETRY`, which accepts every shape and names no SRID. The markers are
`Point`, `LineString`, `Polygon`, `MultiPoint`, `MultiLineString`, `MultiPolygon` and
`GeometryCollection`, plus the `Geography*` spellings; the optional const parameter is the SRID.
A shape Besu does not name — `PointZ`, say — is kept verbatim as `GeometryKind::Other`.

The value stores EWKB exactly as PostGIS sends it, so a round trip is byte-for-byte lossless:

```rust
let point = Geometry::from_hex("0101000020E6100000...")?;
assert_eq!(point.srid(), Some(4326));
assert_eq!(point.kind(), Some(besu::GeometryKind::Point));
assert_eq!(point.ewkb(), &point.clone().into_ewkb()[..]);
```

`Geometry::from_ewkb` and `ewkb()` are the raw form; `from_hex`/`to_hex` and `Display`/`FromStr`
are the hex form PostGIS's output function writes. The driver binds the EWKB as an untyped
parameter, because PostGIS's OID is assigned when the extension is created and there is no stable
OID to declare, and decodes through the raw bytes for the same reason.

`push`/`pull` skip tables, views and sequences that belong to an extension, so PostGIS's own
`spatial_ref_sys` and `topology` objects are not seen as drift.

## Vectors

`pgvector`'s `VECTOR(n)` is a `besu::Vector`, a `Vec<f32>` wrapper:

```rust
use besu::Vector;

pub any: Vector,                                        // VECTOR
#[besu(sql_type = "besu::VectorOf<3>")]
pub xyz: Vector,                                        // VECTOR(3)
```

A `Vec<f32>` field stays a PostgreSQL `REAL[]`; `Vector` is what says "vector column". The
dimension is part of the declared type, so it is reached through `VectorOf<n>` and carried as
`TypeDef::Vector(n)`; a bare `VECTOR` accepts any dimension and introspects as `Vector(0)`.

The parameter is sent untyped in `pgvector`'s binary form — a two-byte dimension, two unused
bytes and the components as big-endian `f32`s — so the server infers `vector(n)` from the column
and checks the dimension itself. A result is decoded from the same binary form; if a server
returns the `[1,2,3]` text instead, that spelling is parsed too. `Display` and `FromStr` write and
read `[1,2,3]`.

The `halfvec` and `sparsevec` types are not modelled: they come back as `Custom("halfvec")` and
`Custom("sparsevec")`, round-trip through DDL, and are read through `sql!`.

## Custom types

Composite types, `tsvector` and an extension type Besu does not model — `halfvec`, `sparsevec`,
`hstore` — come back as `Custom(name)`. The name is preserved and each dotted part is quoted, so
`MySchema.Mood` reaches the type it names rather than folding to lowercase. There is no Rust type
for these, so read them through `sql!`.

Enums, domains, intervals, network addresses, bit strings, ranges, PostGIS geometries and
`pgvector` vectors are not `Custom`: they have their own `TypeDef` variants and values that
round-trip.

A column over a PostgreSQL enum declared by hand is `Custom("mood")`; the type itself lives in
`Schema::enums`, which is introspected, diffed and rendered as `CREATE TYPE`. A derived enum
contributes its type to `Schema::enums` at declaration time, so it reaches the same place without a
snapshot.

## Domains

A domain is a named base type. Both halves are modelled: the column's type is
`TypeDef::Domain { name, base }`, DDL declares the column with the domain's name, and a value is
decoded and written through the base type's binary format.

```sql
CREATE DOMAIN positive_integer AS integer CHECK (VALUE > 0)
```

A column over it introspects as `Domain("positive_integer", Int4)`, and a `i32` decodes from it
because the wire format is `INTEGER`'s. `push` creates the domain before the table that names it,
and a snapshot taken with `pull` replays into an empty database.

Two limitations are deliberate. The constraints, default and `NOT NULL` a domain can carry are
not part of the IR, so a domain created from a snapshot has only its name and base type; an
existing domain is never altered. And a domain built on another domain records the inner domain's
name as its base, because the IR keeps one level; its values still decode, because the wire format
is the innermost base's.

## Gotchas

- `NUMERIC(20,0)` and `NUMERIC(39,0)` are read back as `Decimal`, not guessed to be a `u64` or
  `u128`: a twenty-digit numeric can exceed `u64::MAX`.
- A `CHAR(n)` value is blank-padded by the server, so `char(2)` holding `'a'` reads back as
  `'a '`.
- `DateTime<Local>` is declared `TIMESTAMPTZ` by name matching and then fails to compile when a
  query returns the model. Use `DateTime<Utc>`.
- `TIMETZ` can be created, introspected and rendered, but not read into a Rust value.
- An enum or domain column round-trips through `push`/`pull`. An enum is declared with
  `#[derive(SqlEnum)]`; a domain is not expressible by the derive, so it comes from a snapshot
  (or from a hand-written migration) and its column is decoded through the base type. Dropping
  either is destructive and is refused without `push_destructive`.
- A column with an explicit `COLLATE` is part of its type: a declared `String` will report drift
  against it, exactly as it should, because the two columns sort differently.
- A `NUMERIC` with no precision, such as a bare `numeric` column, introspects as
  `Custom("numeric")` rather than a decimal.
- An interval with a field qualifier, such as `INTERVAL DAY TO SECOND`, keeps its full spelling
  as `Custom`; only the plain `INTERVAL` is modelled.
- A discrete range is canonicalised by the server: `[1,5]` in an `int4range` reads back as
  `[1,6)`. The value is right; only the spelling changed.
- PostGIS and `pgvector` are extensions. A database without them rejects the DDL, and Besu's
  introspection reports a column it cannot decode when the extension is missing from the
  connection's search path.
- A `GEOMETRY` column with a shape or SRID is a different type from a bare `GEOMETRY`, so a
  declared bare `Geometry` field against a `geometry(Point,4326)` column reports drift.
