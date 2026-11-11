# Defining a schema

`#[derive(Table)]` turns one plain struct into everything Besu needs: a row type, a table
handle, an insert struct and a `&'static TableDef`. There is no separate schema file and no
second source of truth: the struct *is* the schema, and migrations diff that metadata against
a live database.

```rust
use besu::prelude::*;

#[derive(Table)]
#[besu(table = "users")]
pub struct User {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    #[besu(unique, column = "email_address")]
    pub email: String,
    pub name: String,
    pub age: i32,
    #[besu(index)]
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[besu(default = true)]
    pub active: bool,
    #[besu(references = "teams.id", on_delete = "cascade")]
    pub team_id: Option<i64>,
    #[besu(skip)]
    pub cached_score: f64,
}
```

## What the derive generates

Given a struct called `User`, the macro emits:

| Item | Notes |
| --- | --- |
| `User` | Unchanged as a type, but it gains `Decode<Dr>` for every driver | 
| `UserTable` | The table handle's type, and the type-level identity used by `Column` |
| `users` | A `const` handle; one field per column, each a `Column<UserTable, S, N>` |
| `UserInsert` | The insert struct returned by `users.insert(..)`, implementing `Insertable` |
| `UserTable::DEF` | A `&'static TableDef` with names, types, indexes and foreign keys; also `users.def()` |

The handle is what makes the query API read like SQL. `users.age` is a copyable value carrying
three things: the SQL name `"age"`, the SQL type `Int4`, and nullability. Copying it does not
allocate; there is no map lookup and no string formatting at the call site.

Column references expose their metadata directly:

```rust
# use besu::prelude::*;
# #[derive(Table)] #[besu(table = "users")] struct User { pub id: i64, pub bio: Option<String> }
assert_eq!(users.id.name(), "id");
assert_eq!(users.id.type_def(), besu::TypeDef::Int8);
assert!(!users.id.nullable());
assert!(users.bio.nullable());
```

## Naming a SQL type directly

The built-in mapping covers the portable types. When you need a specific one — a sized
`VARCHAR`, an exact `DECIMAL`, or a `SqlType` of your own — name it with `sql_type`:

```rust
#[derive(Table)]
pub struct Price {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    #[besu(sql_type = "besu::Decimal<10, 2>")]
    pub amount: String,
    #[besu(sql_type = "besu::VarChar<64>")]
    pub label: String,
}
```

The field's Rust type must be the `Rust` type of the named `SqlType` — `String` for
`Decimal`, `Char` and `VarChar`. A type of your own is a marker that implements `SqlType`,
`IntoSqlValue` and `Decode`.

One caveat: SQLite has no decimal type, and `NUMERIC` affinity would round an exact value, so
a `Decimal` column is declared `TEXT` there. On PostgreSQL and MySQL it is `NUMERIC`/`DECIMAL`,
but decoding one into a `String` is not implemented yet.

## Struct-level attributes

| Attribute | Meaning | Default |
| --- | --- | --- |
| `#[besu(table = "users")]` | The SQL table name | Struct name in `snake_case` |
| `#[besu(name = "users")]` | Alias for `table` | — |
| `#[besu(schema = "public")]` | An optional schema/namespace, stored on `TableDef` | `None` |

The handle identifier is derived from the table name: `table = "user_profiles"` generates the
const `user_profiles`. Two tables with the same SQL name in the same module generate the same
identifier and collide; that is a compile error rather than a silent merge.

## Field attributes

| Attribute | Meaning |
| --- | --- |
| `#[besu(primary_key)]` / `#[besu(primary)]` | Part of the primary key. Several fields make a composite key |
| `#[besu(auto_increment)]` / `#[besu(autoincrement)]` | The database generates the value |
| `#[besu(unique)]` | A `UNIQUE` constraint on this column |
| `#[besu(column = "name")]` / `#[besu(name = "name")]` | The SQL column name; defaults to the field name |
| `#[besu(index)]` | A single-column, non-unique index on this column |
| `#[besu(default = <literal>)]` | A literal default: string, integer, float or boolean |
| `#[besu(default_sql = "expr")]` | A raw SQL default expression, inserted verbatim |
| `#[besu(default_now)]` | `DEFAULT CURRENT_TIMESTAMP` |
| `#[besu(renamed_from = "old_name")]` | Declares that this column used to be called something else, so a migration renames it instead of dropping and recreating it |
| `#[besu(references = "table.column")]` | A foreign key to one column of another table |
| `#[besu(on_delete = "cascade")]` | The foreign key's `ON DELETE` action |
| `#[besu(on_update = "restrict")]` | The foreign key's `ON UPDATE` action |
| `#[besu(skip)]` | Not a column at all; the field must implement `Default` |

Anything else is a compile error with the list of accepted names, which matters because a
typo in `#[besu(primay_key)]` would otherwise silently produce a table with no primary key.

### Defaults

`default` accepts only a literal, because the value is stored in a `&'static TableDef`:

```rust
#[derive(Table)]
pub struct Post {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    #[besu(default = false)]
    pub published: bool,
    #[besu(default = 0)]
    pub views: i32,
    #[besu(default_sql = "gen_random_uuid()")]
    pub public_id: uuid::Uuid,
    #[besu(default_now)]
    pub created_at: chrono::DateTime<chrono::Utc>,
}
```

The metadata holds a `DefaultValue` (`Null`, `Bool`, `Int`, `Float`, `Text`, `Raw` or `Now`).
Each dialect renders it its own way; MySQL 8 wraps defaults on `TEXT`, `BLOB` and `JSON` in
parentheses, for example.

A default changes what `insert` requires, not the Rust type. `published` is still a `bool`,
not an `Option<bool>`: if you insert without it, the database supplies `false`.

### Indexes and uniqueness

`unique` sets the column's `unique` flag and is rendered as an inline `UNIQUE`. `index` emits
a separate `Index` named `{table}_{column}_idx`:

```rust
// Index { name: "users_email_address_idx", columns: &["email_address"], unique: false }
#[besu(index, column = "email_address")]
pub email: String,
```

There is no attribute for a composite index. A `TableDef` is a plain public struct, so a
hand-written definition can declare one, and raw SQL always works. Composite uniqueness works
the same way: only a single column can be marked `unique` by the derive, while a
`besu_schema::Unique` (name plus columns) covers the rest and is rendered as
`CONSTRAINT ... UNIQUE (...)`.

A database that already has a composite `UNIQUE` reports it as a `Unique`, not an index.
That distinction matters: PostgreSQL owns the index that backs the constraint, so
`DROP INDEX` fails on it and only `ALTER TABLE ... DROP CONSTRAINT` removes it.

### Enums

Only PostgreSQL has named enum types that exist outside a column. They are introspected into
`Schema::enums` and diffed (`CREATE TYPE`, `DROP TYPE`, `ALTER TYPE ... ADD VALUE`), but the
derive has no attribute for declaring one: a column declared over an enum arrives as
`TypeDef::Custom("mood")`, which is the type name verbatim. Give the type a spelling with
`sql_type` and a custom `SqlType` marker if the model has to read and write it, or use a
snapshot (`besu pull` / `besu push --schema`) as the source of truth for the type list.

MySQL's inline `ENUM(...)` and `SET(...)` stay `Custom`: they belong to a column, not to the
schema, and `Schema::enums` is empty there. `TimeTz` is modelled for PostgreSQL's `TIMETZ`;
no driver decodes it (sqlx's only TIMETZ type is its own `PgTimeTz` struct), so a column of
that type can be created and introspected but not read into a Rust value.

### Foreign keys

```rust
#[besu(references = "users.id", on_delete = "cascade", on_update = "restrict")]
pub author_id: i64,
```

`references` must be `"table.column"`, or `"schema.table.column"` for a table in another
schema; a missing dot is a compile error. The generated constraint is named
`{table}_{column}_fkey`. Accepted actions are:

| Spelling | `ReferenceAction` |
| --- | --- |
| `cascade` | `Cascade` |
| `set_null`, `setnull` | `SetNull` |
| `set_default`, `setdefault` | `SetDefault` |
| `restrict` | `Restrict` |
| `no_action`, `noaction` | `NoAction` |
| anything else | `Default` (no clause emitted) |

Because an unrecognised action falls back to `Default` rather than erroring, a typo in
`on_delete` is easy to miss. Check the generated DDL if the action matters.

A foreign key is one column, because the attribute is one string. The underlying
`ForeignKey` type supports several columns, so composite keys need a hand-written `TableDef`.
The referenced table does not have to be declared in the same `push` call, but it does have to
exist before the constraint is created.

### Skipped fields

```rust
#[derive(Table)]
pub struct User {
    pub id: i64,
    #[besu(skip)]
    pub cached_score: f64,
}
```

A skipped field is absent from the `SELECT` list, the insert struct and the `TableDef`. When a
row is decoded it is filled with `Default::default()`, so the field must implement `Default`.
This is the place for derived or in-memory state that has no column.

## Type mapping

The derive maps Rust field types by their **last path segment**. `String` becomes text;
`Uuid` becomes a UUID; `Value` (as in `serde_json::Value`) becomes JSON. Wrapping a type in
`Option` makes the column nullable and the Rust field type `Option<T>`. A `&str` field is a
reference type, not a path, and is rejected; use `String`.

| Rust field | `SqlType` | `TypeDef` | SQLite DDL | PostgreSQL DDL | MySQL DDL |
| --- | --- | --- | --- | --- | --- |
| `bool` | `Bool` | `Bool` | `BOOLEAN` | `BOOLEAN` | `BOOLEAN` |
| `i8` | `Int1` | `Int2` | `INTEGER` | `SMALLINT` | `SMALLINT` |
| `i16` | `Int2` | `Int2` | `INTEGER` | `SMALLINT` | `SMALLINT` |
| `i32` | `Int4` | `Int4` | `INTEGER` | `INTEGER` | `INT` |
| `i64` | `Int8` | `Int8` | `INTEGER` | `BIGINT` | `BIGINT` |
| `u8` | `UInt1` | `UInt(8)` | `INTEGER` | `SMALLINT` | `TINYINT UNSIGNED` |
| `u16` | `UInt2` | `UInt(16)` | `INTEGER` | `INTEGER` | `SMALLINT UNSIGNED` |
| `u32` | `UInt4` | `UInt(32)` | `INTEGER` | `BIGINT` | `INT UNSIGNED` |
| `u64` | `UInt8` | `UInt(64)` | `INTEGER` | `NUMERIC(20,0)` | `BIGINT UNSIGNED` |
| `u128` | `U128` | `UInt(128)` | `TEXT` | `NUMERIC(39,0)` | `DECIMAL(39,0)` |
| `i128` | `I128` | `Decimal(39,0)` | `TEXT` | `NUMERIC(39,0)` | `DECIMAL(39,0)` |
| `f32` | `Float4` | `Float4` | `REAL` | `REAL` | `FLOAT` |
| `f64` | `Float8` | `Float8` | `REAL` | `DOUBLE PRECISION` | `DOUBLE` |
| `String` | `Text` | `Text` | `TEXT` | `TEXT` | `TEXT` |
| `Vec<u8>` | `Bytes` | `Bytes` | `BLOB` | `BYTEA` | `BLOB` |
| `uuid::Uuid` | `Uuid` | `Uuid` | `BLOB` | `UUID` | `BINARY(16)` |
| `chrono::NaiveDate` | `Date` | `Date` | `TEXT` | `DATE` | `DATE` |
| `chrono::NaiveTime` | `Time` | `Time` | `TEXT` | `TIME` | `TIME` |
| — (no Rust type) | — | `TimeTz` | `TIME` | `TIMETZ` | `TIME` |
| `chrono::NaiveDateTime` | `Timestamp` | `Timestamp` | `TEXT` | `TIMESTAMP` | `DATETIME(6)` |
| `chrono::DateTime<Utc>` | `TimestampTz` | `TimestampTz` | `TEXT` | `TIMESTAMPTZ` | `DATETIME(6)` |
| `serde_json::Value` | `Json` | `Json` | `TEXT` | `JSON` | `JSON` |

Three details are easy to get wrong:

- `i8` is widened to `Int2`, because no supported database has an 8-bit signed integer. The
  Rust value is still `i8`.
- `u64` needs a type wider than a signed 64-bit integer. PostgreSQL uses `NUMERIC(20,0)` and
  the driver encodes the whole `u64` range into it; MySQL uses `BIGINT UNSIGNED`; SQLite has
  only signed 64-bit integers, so binding a `u64` above `i64::MAX` is an encode error at
  runtime rather than a wrapped value.
- `u128` is `NUMERIC(39,0)` on PostgreSQL and `DECIMAL(39,0)` on MySQL, both exact and wide
  enough for `u128::MAX`. SQLite has nothing that wide, so the column is `TEXT` and the
  driver stores the decimal digits; comparisons and ordering on that column are
  lexicographic unless it is cast to a number. `i128` is modelled as `Decimal(39,0)`, which
  renders the same way on every dialect.
- `DateTime` is matched by name, not by its full type. `chrono::DateTime<Local>` is declared
  as `TimestampTz`, but no driver implements `Decode` for it, so a query that returns the
  model does not compile. Use `DateTime<Utc>`.

### Types the derive cannot produce

The `besu::types` module also defines `Char<N>`, `VarChar<N>` and `Decimal<PRECISION, SCALE>`.
They implement `SqlType`, but the derive has no Rust spelling for them, so they are only
reachable by writing a `TableDef` by hand or implementing your own mapping. Custom column
types are on the [roadmap](../roadmap.md).

## Nullability

Nullability is a separate type parameter, not a wrapper around the SQL type:

```rust
pub struct Column<T, S: SqlType, N: Nullability = NonNull>;
```

`NonNull` wraps the Rust type as `T`; `Nullable` wraps it as `Option<T>`. The separation keeps
`Option` from nesting: `Option<Option<T>>` is not a valid column type because the derive
strips exactly one `Option` and then rejects the inner one.

A nullable column has three distinct states in a write, which is the subject of
[Inserting data](insert.md): unset (use the default), explicitly `NULL`, and a value. Reading
a nullable column always yields `Option<T>`, and the column has `is_null` / `is_not_null`
helpers for filtering on it.

## What is checked

| Mistake | When it fails |
| --- | --- |
| A field type with no mapping (`char`, `f16`, an array) | Compile error listing the supported types |
| A generic table (`struct Foo<T>`) | Compile error |
| An enum, union, tuple struct or unit struct | Compile error |
| A table whose every field is `skip`ped | Compile error |
| A nullable primary key or auto-increment column | Compile error |
| A malformed `references` string | Compile error |
| An unknown or misspelled `#[besu(..)]` key | Compile error listing the accepted keys |
| Two tables with the same SQL name in one module | Compile error at the generated handle |
| A database whose schema drifted from the struct | Runtime, caught by `push` unless destructive |

## Gotchas

- Because the type mapping looks at the last path segment, a type alias or a same-named type
  from another crate can be mapped to the wrong SQL type. `time::Date` is not
  `chrono::NaiveDate`, but both end in `Date`.
- `#[besu(schema = "...")]` is stored and used to qualify table names, and it is part of the
  table's identity: `app.users` and `users` are two tables. Leave it off for the database's
  default schema — PostgreSQL reports the current schema as unqualified, so naming it
  explicitly makes the declared table look like a different one. Besu never runs
  `CREATE SCHEMA`, so the schema has to exist. On SQLite a schema is an attached database, so
  only set it if you have attached one; MySQL treats it as a database name.
- `auto_increment` is normally paired with `primary_key`. The derive does not enforce the
  pairing, and some databases reject an auto-increment column that is not a key.
- A generated index or foreign key name can collide with one you created by hand. The naming
  scheme is fixed (`{table}_{column}_idx`, `{table}_{column}_fkey`).
- `#[besu(skip)]` removes the field from both reading and writing. If you want a column that
  the database fills in, use `default` or `auto_increment` instead.
