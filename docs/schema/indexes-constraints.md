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

A table-level `#[besu(index(columns(...)))]` declares anything wider: several columns, an
expression, an operator class, a unique index or an access method. `columns` takes column names
and raw SQL expressions, in key order:

```rust
#[derive(Table)]
#[besu(table = "users")]
#[besu(index(columns(team_id, created_at)))]
#[besu(index(
    columns(lower(email)),
    name = "users_email_lower",
    unique,
    using = "gin",
    opclass = "gin_trgm_ops"
))]
pub struct User {
    #[besu(primary_key)]
    pub id: i64,
    pub team_id: i64,
    pub email: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}
```

```sql
CREATE INDEX "users_team_id_created_at_idx" ON "users" ("team_id", "created_at")
CREATE UNIQUE INDEX "users_email_lower" ON "users" USING gin (lower(email) gin_trgm_ops)
```

The keys inside `index(...)` are `columns(...)`, `name = "..."`, `unique`,
`using = "..."` (`btree`, `gin`, `gist`, `brin`, `fulltext` or `spatial`) and `opclass`. An
operator class may be written once for a single-entry index:

```rust
#[besu(index(columns(email), opclass = "text_pattern_ops"))]
```

or by column name for a composite one:

```rust
#[besu(index(columns(email, lower(name)), opclass(email = "text_pattern_ops")))]
```

`opclass(a = "...")` names a column, not an expression. An expression that needs its own
operator class has no name to match, so declare that index in the `Schema` IR (which attaches
the class to the entry directly) or as a hand-written `TableDef`. A bare `opclass` requires
exactly one entry.

Without a `name`, the index is called `{table}_{column}_idx` for a single column (the same name
`#[besu(index)]` uses), `{table}_{col1}_{col2}_idx` for several, and an expression contributes
`expr{position}`. For example `columns(a, lower(email))` on `users` is `users_a_expr2_idx`.
Naming an index yourself keeps that name, which is what makes introspection converge: the DDL
always writes the explicit name.

An index can also be built by hand in a `Schema`, which is how a partial index gets a predicate:

```rust
use besu::schema::{Index, IndexColumn, IndexKind, Schema};

let mut schema = Schema::from_defs(&[users.def()]);
schema.table_mut("users").unwrap().indexes.push(Index {
    name: "users_name_email_idx".into(),
    columns: vec![IndexColumn::column("name"), IndexColumn::column("email")],
    unique: false,
    kind: IndexKind::Btree,
    predicate: None,
});

db.push_schema(&schema).await?;
```

`IndexColumn::expression("lower(email)")` is the expression spelling, and
`IndexColumn::column("email").with_opclass("text_pattern_ops")` attaches an operator class.
`From<&str>` and `From<String>` produce a plain column, so `vec!["name".into()]` still works.

A `TableDef` is also a plain public struct, so a hand-written const can carry a composite,
expression or operator-class index. It cannot carry a predicate or a check: those live only in
the `Schema` IR. An access method is part of `Index`, so a hand-written `TableDef` can declare
one. Its entries are `besu::IndexEntry`, which has `column`, `expression` and `with_opclass`
`const fn` constructors.

### Access methods

`Index::kind` is the access method, and it is part of the index's identity: changing it is a
drop and a create, because both statements use the same name. The drop is a destructive change, so
`push` reports it and refuses unless `push_destructive` is used. The default is `Btree`, which is
what `#[besu(index)]` declares, and it is left out of a JSON snapshot so a snapshot written before
access methods existed loads unchanged.

```rust
use besu::schema::{IndexColumn, IndexKind};

schema.table_mut("events").unwrap().indexes.push(Index {
    name: "events_payload_idx".into(),
    columns: vec![IndexColumn::column("payload")],
    unique: false,
    kind: IndexKind::Gin,
    predicate: None,
});
```

| `IndexKind` | PostgreSQL | MySQL | SQLite |
| --- | --- | --- | --- |
| `Btree` | `btree` | `BTREE` | b-tree |
| `Gin` | `USING gin` | plain key | b-tree |
| `Gist` | `USING gist` | plain key | b-tree |
| `Brin` | `USING brin` | plain key | b-tree |
| `FullText` | `USING gin` | `FULLTEXT KEY` | b-tree |
| `Spatial` | `USING gist` | `SPATIAL KEY` | b-tree |

```sql
CREATE INDEX "events_payload_idx" ON "events" USING gin ("payload")
```

PostgreSQL reads `pg_am.amname` back, so a live GIN, GiST or BRIN index is reported as itself and
a declared b-tree over it is a real change. An index using `hash` or `spgist` has no `IndexKind`,
so introspection skips it rather than reporting a b-tree that is not there. MySQL reads
`STATISTICS.INDEX_TYPE`; SQLite has one access method and always reports `Btree`.

MySQL's `FULLTEXT` takes no prefix and no `UNIQUE`, and `SPATIAL` needs a `NOT NULL` geometry
column. A `GIN`, GiST or BRIN index pushed to MySQL or SQLite is created as a plain b-tree, which
is the closest thing those databases have; a snapshot that declares one will keep reporting the
difference on each push, so declare the kind that matches the target database.

### MySQL index prefixes

InnoDB limits a key to 3072 bytes, and `utf8mb4` uses up to four bytes per character. MySQL
therefore cannot key a whole `TEXT`, `BLOB` or `VARCHAR(1000)` column, and Besu renders a prefix
instead:

```sql
CREATE TABLE `users` (
    `email` VARCHAR(1000) NOT NULL,
    UNIQUE KEY `email` (`email`(768))
) ENGINE=InnoDB
```

A `UNIQUE` over a prefix is unique only over the prefix, so two values that share their first 768
characters collide. That is MySQL's limit, not a choice Besu can avoid; keep unique strings under
768 characters, or store a hash beside them.

The prefix is part of the table, so adding an index to an existing `TEXT` column needs the column
types too. `push` looks the table up in the schemas it is diffing, which is why
`CREATE INDEX users_email_idx ON users (email(255))` works rather than failing with MySQL error
1170 ("BLOB/TEXT column used in key specification without a key length").

### Partial indexes

Only PostgreSQL reads a predicate back into the IR. `Index::predicate` is the `WHERE` clause,
appended verbatim.

```rust
use besu::schema::IndexColumn;

schema.table_mut("events").unwrap().indexes.push(Index {
    name: "events_errors_idx".into(),
    columns: vec![IndexColumn::column("kind")],
    unique: false,
    kind: IndexKind::Btree,
    predicate: Some("kind = 'error'".into()),
});
```

```sql
CREATE INDEX "events_errors_idx" ON "events" ("kind") WHERE kind = 'error'
```

SQLite exposes no predicate in its catalogue, so a partial index is skipped; MySQL has none.

### Expression indexes

A key entry can be a raw SQL expression instead of a column:

```rust
#[besu(index(columns(lower(email)), name = "users_email_lower_idx"))]
```

```sql
CREATE INDEX "users_email_lower_idx" ON "users" (lower(email))
```

The expression is raw SQL, so it is the caller's responsibility that each target database
accepts it. SQLite takes it as written; PostgreSQL adds the parentheses its grammar requires
around a non-function expression (`(a + b)`), and MySQL 8 spells a functional key part as
`(expression)`, so the same declaration renders
`KEY users_email_lower_idx ((lower(email)))`. An expression is never quoted as an identifier,
and a changed expression is a drop and a create like any other index change.

### Operator classes

An operator class follows its key entry and is PostgreSQL-only. The other dialects omit it
rather than render a statement they reject; because SQLite and MySQL have no operator-class
concept, omitting it does not change the index they build.

```rust
#[besu(index(columns(email), opclass = "text_pattern_ops"))]
#[besu(index(columns(email, lower(name)), opclass(email = "text_pattern_ops")))]
```

```sql
CREATE INDEX "users_email_idx" ON "users" ("email" text_pattern_ops)
```

`text_pattern_ops` is not the type's default operator class, so introspection reports it back.
The default class (`text_ops`, `int4_ops`, ...) is not reported, which is what keeps a plain
`columns(email)` declaration converging instead of looking like a change on every push.

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
| Index, composite | Yes | Yes | Yes |
| Index, expression | Yes | Yes | Yes |
| Index operator class | Yes | Yes | Yes |
| Index access method | Yes | Yes | Yes |
| Partial index | No | No | Yes (PostgreSQL) |
| Foreign key, single column | Yes | Yes | Yes |
| Foreign key, composite | No | Yes | Yes |
| `CHECK` | No | No | Yes |

A table definition pushed from Rust therefore declares its keys, uniques (one column), indexes
(composite, expression, operator class and access method), and its single-column foreign keys.
Everything else needs a hand-written `TableDef`, a `besu pull` snapshot or a migration.

## Generated columns

A column is generated when the database computes it from the others. The derive declares it, and
it is never part of an insert or a whole-row update:

```rust
#[besu(generated = "price * quantity")]
pub total: i64,
```

`stored` defaults to true. Pass `stored = false` for a virtual column, which PostgreSQL does not
implement and creates as stored:

```rust
#[besu(generated = "price + quantity", stored = false)]
pub shift: i64,
```

```sql
CREATE TABLE "line_items" (
    "id" INTEGER PRIMARY KEY,
    "price" INTEGER NOT NULL,
    "quantity" INTEGER NOT NULL,
    "total" INTEGER GENERATED ALWAYS AS (price * quantity) STORED,
    "shift" INTEGER GENERATED ALWAYS AS (price + quantity) VIRTUAL
)
```

The expression is raw SQL and Besu cannot check it, so it is your responsibility that all three
dialects accept it. A generated column cannot be a primary key, auto-increment, `unique` or carry
a default: at least one dialect rejects each of those.

PostgreSQL and MySQL report the expression in `information_schema.columns`; SQLite has no
catalogue view of it, so it is read from the `CREATE TABLE` text that `sqlite_master` stores.
Comparison folds identifier quotes and `::type` casts away, so `to_tsvector('english', bio)`
converges against the `to_tsvector('english'::regconfig, bio)` PostgreSQL reports.

## Safety

Adding an index, unique, foreign key or check is `Risky`: it validates existing rows. Changing any
part of an index — its key entries, an operator class, uniqueness, the access method or a
predicate — is reported as a drop plus a create, because both statements use the same name.
Dropping an index, unique or check is `Destructive`; dropping a foreign key is only `Risky`.
Adding a generated column is `Safe`, because there is no value to invent: the database computes
one for every row.

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
- An index with `INCLUDE` columns and a PostgreSQL exclusion constraint are invisible to the IR:
  introspection skips them, so `push` never drops one. So is an index using `hash` or `spgist`,
  which has no `IndexKind`.
- SQLite introspection reads an expression key part from the `CREATE INDEX` text in
  `sqlite_master`. A partial index is still skipped, because the IR has no predicate for one on
  SQLite, and an index whose expression cannot be matched to its key part is skipped rather than
  reported as a smaller index.
- MySQL introspects a functional index from `STATISTICS.EXPRESSION` and reports the expression in
  MySQL's own backticked spelling; comparison folds identifier quoting away.
- A `FULLTEXT` or `SPATIAL` index pushed to PostgreSQL becomes a GIN or GiST index, and a GIN,
  GiST or BRIN index pushed to MySQL or SQLite becomes a plain b-tree. Introspection reports the
  kind that is really there, so declare the kind the target database supports.
- Index and foreign-key names are fixed by the derive; a hand-written object with the same name
  collides.
