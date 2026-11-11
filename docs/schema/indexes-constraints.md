# Indexes and constraints

Primary keys, uniqueness, indexes, foreign keys and checks are schema objects: `push` diffs them
and the same DDL renders them. The derive can declare some; the rest need a snapshot or a migration.

## Primary keys

One field marked `primary_key` is an inline `PRIMARY KEY`; two or more are a table-level
constraint in declaration order.

```rust
#[besu(primary_key)]
pub user_id: i64,
#[besu(primary_key)]
pub team_id: i64,
```

```sql
CREATE TABLE "memberships" (
    "user_id" BIGINT NOT NULL,
    "team_id" BIGINT NOT NULL,
    PRIMARY KEY ("user_id", "team_id")
)
```

A primary key column is implicitly `NOT NULL`. `TableDef::primary_key()` iterates the key columns
and `has_composite_primary_key()` answers the question the DDL generator asks.

## Unique constraints

A single-column `UNIQUE` is a flag on the column, rendered inline as `"email" TEXT NOT NULL UNIQUE`.

```rust
#[besu(unique)]
pub email: String,
```

Anything wider is a `Unique` — a name and a column list — because a constraint over
several columns has no per-column spelling, and the derive has no attribute for one:

```rust
use besu::schema::Unique;

schema.table_mut("memberships").unwrap().uniques.push(Unique {
    name: "memberships_user_team_key".into(),
    columns: vec!["user_id".into(), "team_id".into()],
});
```

```sql
ALTER TABLE "memberships" ADD CONSTRAINT "memberships_user_team_key" UNIQUE ("user_id", "team_id")
```

On PostgreSQL that constraint owns a backing index, so `DROP INDEX` cannot remove it; the drop is
`ALTER TABLE ... DROP CONSTRAINT`. SQLite has no `ALTER TABLE ... ADD CONSTRAINT`, so a composite
unique is written inside `CREATE TABLE` and adding or dropping one afterwards renders nothing.
MySQL has no object separate from the unique key, and introspects a multi-column unique as a
`Unique` keyed by that index.

## Indexes

`#[besu(index)]` declares a single-column, non-unique index named `{table}_{column}_idx`.

```rust
#[besu(index)]
pub created_at: chrono::DateTime<chrono::Utc>,
```

```sql
CREATE INDEX "users_created_at_idx" ON "users" ("created_at")
```

A composite index has no attribute, but the IR expresses one, so a hand-built `Schema` can declare
it — `Schema::from_defs` gives you the tables and `table_mut` lets you push onto the list:

```rust
use besu::schema::{Index, Schema};

let mut schema = Schema::from_defs(&[users.def()]);
schema.table_mut("users").unwrap().indexes.push(Index {
    name: "users_name_email_idx".into(),
    columns: vec!["name".into(), "email".into()],
    unique: false,
    predicate: None,
});

db.push_schema(&schema).await?;
```

```sql
CREATE INDEX "users_name_email_idx" ON "users" ("name", "email")
```

A `TableDef` is also a plain public struct, so a hand-written const can carry a composite index or
a multi-column foreign key. It cannot carry a unique, a predicate or a check: those live only in
the `Schema` IR.

### Partial indexes

Only PostgreSQL reads a predicate back into the IR. `Index::predicate` is the `WHERE` clause,
appended verbatim.

```rust
schema.table_mut("events").unwrap().indexes.push(Index {
    name: "events_errors_idx".into(),
    columns: vec!["kind".into()],
    unique: false,
    predicate: Some("kind = 'error'".into()),
});
```

```sql
CREATE INDEX "events_errors_idx" ON "events" ("kind") WHERE kind = 'error'
```

SQLite introspects no predicate for a partial index, so introspection skips one; MySQL has none.

## Foreign keys

`references` declares a single-column foreign key named `{table}_{column}_fkey`.

```rust
#[besu(references = "users.id", on_delete = "cascade", on_update = "restrict")]
pub author_id: i64,
```

```sql
CONSTRAINT "posts_author_id_fkey"
    FOREIGN KEY ("author_id") REFERENCES "users" ("id")
    ON DELETE CASCADE ON UPDATE RESTRICT
```

The accepted action spellings are listed in [schema declaration](declaration.md); an
unrecognised one silently becomes "no clause". A composite foreign key has no attribute, but the
`ForeignKey` type holds a column list, so it arrives the same way a composite index does — from
a `Schema` or a hand-written `TableDef`.

The referenced table does not have to be in the same `push` call, but it has to exist before the
constraint is created; `push` creates tables in dependency order.

## CHECK constraints

A check is a name and the boolean expression inside `CHECK (...)`. The derive has no attribute
for one, and `TableDef` has no field for one.

```rust
use besu::schema::Check;

schema.table_mut("accounts").unwrap().checks.push(Check {
    name: "accounts_balance_check".into(),
    expression: "balance >= 0".into(),
});
```

```sql
ALTER TABLE "accounts" ADD CONSTRAINT "accounts_balance_check" CHECK (balance >= 0)
```

PostgreSQL is the only dialect that round-trips a check: it is read from `pg_constraint`, the extra
parentheses are normalised away, and adding or dropping one is a plain `ALTER TABLE`. SQLite keeps
a check inside the `CREATE TABLE` text and MySQL reports a `CHECK_CLAUSE` it will not accept back.

## What the derive can declare

| Object | Derive | Hand-written `TableDef` | `Schema` snapshot |
| --- | --- | --- | --- |
| Primary key, single or composite | Yes | Yes | Yes |
| `UNIQUE`, single column | Yes | Yes | Yes |
| `UNIQUE`, composite | No | No | Yes |
| Index, single column | Yes | Yes | Yes |
| Index, composite | No | Yes | Yes |
| Partial index | No | No | Yes (PostgreSQL) |
| Foreign key, single column | Yes | Yes | Yes |
| Foreign key, composite | No | Yes | Yes |
| `CHECK` | No | No | Yes |

A table definition pushed from Rust therefore declares its keys, single-column uniques and indexes,
and its single-column foreign keys. Everything else needs a hand-written `TableDef`, a `besu pull`
snapshot or a migration.

## Safety

Adding an index, unique, foreign key or check is `Risky`: it validates existing rows. Dropping an
index, unique or check is `Destructive`; dropping a foreign key is only `Risky`.

## Gotchas

- SQLite cannot add or drop a composite `UNIQUE`, a foreign key or a check on an existing table.
  The change is reported and renders no statement; only a `CREATE TABLE` — or a table rebuild
  caused by a type change — writes one.
- MySQL renders an `ADD CONSTRAINT ... CHECK` but never introspects one, so a check in a snapshot
  is applied once and then reported again on the next push, where the duplicate name fails.
- SQLite does not report a constraint's own name, so uniques are matched by their columns rather
  than by name. Renaming a unique in a snapshot is not a change.
- MySQL stores a single-column `UNIQUE` as an index named after the column and introspects it back
  onto the column; a unique index you name yourself stays an index.
- An expression index, an index with `INCLUDE` columns and a PostgreSQL exclusion constraint are
  invisible to the IR: introspection skips them, so `push` never drops one.
- Index and foreign-key names are fixed by the derive; a hand-written object with the same name
  collides.
