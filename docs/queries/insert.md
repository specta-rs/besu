# Insert

`db.insert(table)` starts an `INSERT`. Add one row with `value`, many with `values`, and await
it to run. Without `returning`, the statement resolves to the driver's output; with it, the
inserted rows come back.

```rust
let created: Vec<User> = db
    .insert(users)
    .value(users.insert("oscar@example.com", "Oscar", 21))
    .returning(users)
    .await?;
```

## `value` and `values`

`value` takes the table's insert struct; `values` takes any iterator of them and emits one
multi-row statement.

```rust
db.insert(users)
    .value(users.insert("oscar@example.com", "Oscar", 21))
    .await?;

db.insert(posts)
    .values([
        posts.insert("Hello, world", oscar.id),
        posts.insert("A second post", oscar.id),
    ])
    .await?;
```

```sql
INSERT INTO "users" ("email", "name", "age") VALUES (?, ?, ?)
INSERT INTO "posts" ("title", "author_id") VALUES (?, ?), (?, ?)
```

## What the derive generates

```rust
#[derive(Table)]
#[besu(table = "users")]
pub struct User {
    #[besu(primary_key, auto_increment)] pub id: i64,
    #[besu(unique)] pub email: String,
    pub name: String,
    pub age: i32,
    pub bio: Option<String>,
}
```

The macro emits a `UserInsert` with one `Set` field per column, and a constructor with one
argument per required column:

```rust
pub struct UserInsert {
    pub id: Set<i64>, pub email: Set<String>, pub name: Set<String>,
    pub age: Set<i32>, pub bio: Set<String>,
}

impl UserTable {
    pub fn insert(email: impl Into<String>, name: impl Into<String>, age: impl Into<i32>)
        -> UserInsert
    {
        todo!()
    }
}
```

A column is required when it is **not nullable, not auto-incrementing and has no default**;
everything else starts as `Set::Unset` and is left to the database. Every column also has a
setter, for the optional ones:

```rust
users.insert("oscar@example.com", "Oscar", 21).bio("Rust");        // writes 'Rust'
users.insert("oscar@example.com", "Oscar", 21).bio(None::<String>); // writes NULL
```

## `Set` has three states

| State | `INSERT` behaviour |
| --- | --- |
| `Set::Unset` | The column is omitted, so the database default applies |
| `Set::Null` | The column is written as an explicit `NULL` |
| `Set::Value(v)` | The column is written with the bound value `v` |

The setters decide which one you get: `T` becomes `Value`, `&str` becomes `Value(String)`, and
`Option<T>` becomes `Value` or `Null`. `Set::is_set()` reports whether anything was provided.

## The column list is the union of what the rows provide

A bulk insert keeps a column when at least one row provides it; a row that leaves it unset is
filled with `DEFAULT`. Not every dialect accepts that inside `VALUES`:

```text
unsupported: `sqlite` does not support `DEFAULT` inside `VALUES`, and this bulk insert
provides `bio` in some rows but not others
```

SQLite sets `SUPPORTS_DEFAULT_IN_VALUES = false`; PostgreSQL and MySQL set it to `true`. Give
every row the same columns when targeting SQLite, or insert in separate statements.

## `returning`

`returning` takes the same selections as [`pick`](select.md#projection): the table handle, a
column, an expression or a tuple such as `(users.id, users.name)`. `returning(users)` is the whole row, including generated
columns, which is the reliable way to learn the `id` an auto-increment column produced.

```rust
let ids: Vec<i64> = db
    .insert(users)
    .value(users.insert("a@example.com", "Alice", 30))
    .returning(users.id)
    .await?;
```

```sql
INSERT INTO "users" ("email", "name", "age") VALUES (?, ?, ?) RETURNING "users"."id"
```

`RETURNING` is a dialect capability. PostgreSQL and SQLite support it; MySQL does not, and the
failure is a build error rather than a compile error:
``unsupported: `mysql` does not support `RETURNING` ``.

## Upserts

`on_conflict` starts an `ON CONFLICT` clause. It takes a column, or a tuple of up to four
columns, as the target.

```rust
db.insert(users)
    .value(users.insert("oscar@example.com", "Oscar", 22))
    .on_conflict(users.email)
    .do_update(users.age, 22)
    .await?;
```

```sql
INSERT INTO "users" ("email", "name", "age") VALUES (?, ?, ?)
ON CONFLICT ("email") DO UPDATE SET "age" = ?
```

`.do_nothing()` is the other action, and `on_conflict` alone is already `DO NOTHING`.
`do_update` may be called more than once, and a composite unique key uses a tuple,
`.on_conflict((users.email, users.team_id))`. Besu checks the columns exist, not that the
database has a unique constraint over them.

MySQL spells the clause differently: ``INSERT ... ON DUPLICATE KEY UPDATE `age` = ?``. It has no
target on its upsert, so the value passed to `on_conflict` is ignored there, and it has no safe
equivalent of `DO NOTHING` (`INSERT IGNORE` swallows other errors), so `do_nothing` is a build
error on MySQL.

## Inserting a model

`UserInsert` implements `From<User>`, so an existing row can be inserted again:

```rust
let copy: UserInsert = user.clone().into();
db.insert(users).value(copy).await?;
```

An auto-increment column becomes `Unset`, a nullable column takes `Value` or `Null` from its
`Option`, and every other column becomes `Value` — so a `None` in the model is written as
`NULL`, not left to a default.

## Beyond the parameter limit

Every value is a bound parameter, and each dialect has a ceiling: `MAX_PARAMS` is 32,766 on
SQLite and 65,535 on PostgreSQL and MySQL. An insert that would exceed it is a build error that
names the largest batch that fits, and `values_chunked` does the splitting, returning one driver output per statement:

```rust
let outputs = db.insert(users).values_chunked(rows, 5_000).await?;
```

The batches are **not** atomic: wrap the call in `db.transaction(..)` when all of them must
succeed or none. `returning` is unavailable here, because there is no single statement to
attach it to.

## What is checked

| Mistake | When it fails |
| --- | --- |
| A missing or mistyped required argument | Compile error at `users.insert(..)` |
| Setting a column to a value of the wrong type | Compile error at the setter |
| Inserting zero rows | Build error |
| More parameters than the dialect allows | Build error with a suggested batch size |
| `RETURNING` or `do_nothing` on MySQL | Build error |
| `do_update` with no target on PostgreSQL or SQLite | Build error |

## Gotchas

- **`do_update` without `on_conflict` compiles and then fails** on PostgreSQL and SQLite with
  ``ON CONFLICT needs a target column; call `.on_conflict(column)` first``.
- **Do not write `.into()` on insert arguments.** The parameter is `impl Into<String>`, so the
  target type is not concrete and the compiler sees several `From` implementations.
- **`to_sql()` is not available once you call `returning`.** The builder changes type, so assert
  the plain insert instead.
- **A `NOT NULL` column with `#[besu(default = ..)]` is optional too**, and an upsert target
  must match a real unique constraint — the columns are checked, the constraint is not.
