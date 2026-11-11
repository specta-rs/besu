# Migrations

Migrations are built on one idea: the schema your code declares and the schema a live
database has are the same type. That is what makes `push` a two-line operation, and what makes
drift between the two impossible to miss.

## The schema IR

`besu_schema::Schema` is a serialisable description of a whole database:

```rust
pub struct Schema {
    pub tables: Vec<Table>,
    pub enums: Vec<Enum>,
}

pub struct Table {
    pub name: String,
    pub schema: Option<String>,
    pub columns: Vec<Column>,
    pub indexes: Vec<Index>,
    pub uniques: Vec<Unique>,      // multi-column UNIQUE constraints
    pub foreign_keys: Vec<ForeignKey>,
    pub comment: Option<String>,
}

pub struct Unique {
    pub name: String,
    pub columns: Vec<String>,
}

pub struct Enum {
    pub name: String,
    pub values: Vec<String>,
}

pub struct Column {
    pub name: String,
    pub ty: TypeDef,
    pub nullable: bool,
    pub auto_increment: bool,
    pub primary_key: bool,
    pub unique: bool,
    pub default: Option<String>,   // a normalised SQL expression
    pub comment: Option<String>,
}
```

A table's identity is `(schema, name)`, so two tables called `users` in different schemas are
two tables, and `Schema::find(schema, name)` is how the differ pairs them up. A single-column
`UNIQUE` stays on `Column::unique`; a multi-column one is a `Unique`, because it is a
constraint the database owns rather than an index Besu can drop.

Three things produce a `Schema`:

- `#[derive(Table)]` metadata, via `Database::schema(tables)` or `Schema::from_defs(..)`.
- A live database, via `Database::introspect()`.
- A JSON snapshot on disk, via `Schema::load(..)`.

And two things consume it: `diff` (or `diff_with`) and `Ddl`. The whole pipeline is:

```text
#[derive(Table)]  ─┐
live database     ─┼──▶ Schema ──▶ diff ──▶ DDL ──▶ database
snapshot on disk  ─┘
```

Ordering is deterministic on purpose. `Schema::sort()` sorts tables by schema and name, sorts
enums by name, and normalises each table (primary-key columns first, indexes, uniques and
foreign keys sorted by name), and `topological_order()` returns tables after the tables they
reference. That means two equivalent schemas produce the same DDL and the same JSON, which is
what makes snapshots reviewable.

The diff's application order is fixed too: `CREATE TYPE` before the `CREATE TABLE` that uses
the type, additive changes before destructive ones, and `DROP TYPE` after the last table that
used it.

### Normalisation matters

`diff` compares types literally. `diff_with(current, desired, normalise)` takes a function and
is what `push` uses, passing `Ddl::normalise_type` from the dialect. A dialect whose storage
collapses several canonical types therefore supplies its own folding:

| Dialect | Folds |
| --- | --- |
| SQLite | `Int2`/`Int4`/`Int8` and every `UInt` → `Int8`; `Char`/`VarChar` → `Text`; `Jsonb` → `Json`; `TimestampTz` → `Timestamp`; `TimeTz` → `Time` |
| PostgreSQL | `UInt(8)` → `Int2`; `UInt(16)` → `Int4`; `UInt(32)` → `Int8`; `UInt(64)` → `Decimal(20, 0)` |
| MySQL | `TimeTz` → `Time` — otherwise nothing, it stores the declared types faithfully |

So on SQLite, changing an `i32` field to `i64`, or a `VARCHAR(50)` to `TEXT`, is not reported
as a change. On PostgreSQL, changing `u8` to `i16` is invisible because both are stored as
`SMALLINT`. That avoids a diff that never converges; the trade-off is that a genuine widening
is invisible to `push`.

Defaults are normalised separately, and universally: surrounding quotes and whitespace are
stripped, and `now()`, `current_timestamp()` and `current_timestamp` all compare equal.

## `push`

```rust
let result = db.push([users.def(), posts.def()]).await?;

if result.is_empty() {
    println!("already up to date");
} else {
    for statement in &result.statements {
        println!("{statement};");
    }
}
```

`push` does three things:

1. Introspects the live database into a `Schema`.
2. Diffs it against the declared schema.
3. Refuses destructive changes, then renders and executes the rest.

It is safe to call on every boot because an up-to-date schema produces no changes. The
`PushResult` reports both the abstract changes and the exact statements:

```rust
pub struct PushResult {
    pub changes: Vec<Change>,
    pub statements: Vec<String>,
}
```

`is_empty()` means nothing ran. `is_destructive()` means at least one change was classified
as `Destructive`.

`create_schema` is the blunter alternative: it emits `CREATE TABLE` for every declared table
(and then its secondary indexes) without introspecting, so it fails if a table already
exists. Use it for a fresh database, not for convergence.

### Safety classification

Every `Change` carries a `Safety` level:

| Change | Safety | Why |
| --- | --- | --- |
| `CreateTable` | Safe | Adds a table |
| `CreateEnum` | Safe | Adds a type nothing uses yet |
| `AddColumn` (nullable or with a default) | Safe | Existing rows get a value |
| `AddColumn` (`NOT NULL`, no default) | Risky | Fails on a non-empty table |
| `AddIndex` | Risky | Rewrites data, but is reversible |
| `AddUnique` | Risky | Validates existing rows |
| `AddForeignKey` | Risky | Validates existing rows |
| `AddEnumValue` | Risky | Appending cannot break a row, but rewrites the type |
| `AlterColumn` (same type/nullable/default/auto-increment) | Safe | Metadata-only |
| `AlterColumn` (anything else) | Risky | Rewrites the column |
| `DropForeignKey` | Risky | Loses a constraint, not data |
| `DropColumn` | Destructive | Loses data |
| `DropIndex` | Destructive | Loses an index (and possibly uniqueness) |
| `DropUnique` | Destructive | Loses a uniqueness guarantee |
| `DropTable` | Destructive | Loses data |
| `DropEnum` | Destructive | Nothing can read a row still holding the type |
| `RemoveEnumValue` | Destructive | Rows holding that value can no longer be read |

`push` rejects the whole plan if any change is `Destructive`, and returns
`SchemaError::Destructive(changes)` listing them. `Risky` changes are allowed: they can be
slow or fail on existing data, but they do not silently throw data away.

```rust
// Applying a rename is a drop plus an add, so it is refused:
let err = db.push([users.def()]).await.unwrap_err();

// Only say this if you mean it:
db.push_destructive([users.def()]).await?;
```

The `push_schema` / `push_schema_destructive` variants take a `&Schema` instead of table
definitions, which is what the CLI uses for a snapshot.

### Dialect limits

Before executing anything, `push` also asks the DDL dialect whether it can alter a column in
place. PostgreSQL and MySQL can. SQLite cannot, so it *rebuilds the table* instead: create the
new definition under a temporary name, copy the columns that exist in both old and new, drop
the old table, rename, and recreate the indexes. Foreign keys are disabled around that
sequence, which is why `SchemaDriver::execute_statements` exists — the pragma is per
connection, so every statement of a rebuild has to reach the same one.

A dialect that can do neither returns `false` from `can_alter_column`, and `push` refuses
rather than falling back to the portable default `alter_column`, which is a `DROP COLUMN`
followed by an `ADD COLUMN` and loses the column's data.

### Enums, uniques and schemas

**Enums.** Only PostgreSQL has named types that are not part of a column. `Ddl::create_enum`,
`drop_enum` and `add_enum_value` render `CREATE TYPE ... AS ENUM`, `DROP TYPE` and
`ALTER TYPE ... ADD VALUE`; the default implementations return no statements, because MySQL
spells an enum inline in the column and SQLite has none at all. A column whose type is an enum
comes back as `TypeDef::Custom("mood")`, and the type itself is listed in `Schema::enums` and
diffed.

`ALTER TYPE ... ADD VALUE` cannot run inside a transaction block before PostgreSQL 12. `push`
runs statements one at a time, so it is fine there; a migration file that wraps several
statements in a transaction may need the `ALTER TYPE` in a migration of its own.

**Composite `UNIQUE`.** A single-column `UNIQUE` is on the column. Anything wider is a
`Unique` (name plus columns), added with `ALTER TABLE ... ADD CONSTRAINT ... UNIQUE` and
removed with `ALTER TABLE ... DROP CONSTRAINT`, which is the statement PostgreSQL actually
accepts — the index behind the constraint cannot be dropped on its own. SQLite has no
`ALTER TABLE ... ADD CONSTRAINT`, so it renders uniques inside `CREATE TABLE` and returns no
statements for adding or dropping one afterwards. Uniques are matched by the columns they
cover rather than by name, because SQLite does not report a constraint's name.

**Schemas.** `Table::schema` is part of a table's identity. PostgreSQL introspection reports
`None` for a table in `current_schema()` and the schema name for one anywhere else, so a table
declared with `#[besu(schema = "app")]` and the unqualified table of the same name are
distinct. A schema must already exist: Besu qualifies names and creates tables in it, but
does not run `CREATE SCHEMA`.

`push` manages the default schema and every schema the declared schema names. A table in a
schema that is never mentioned is left alone rather than reported as a `DropTable`, so a
database that holds more than one application's objects does not make every push
destructive. Drop the last table of a schema from your code and the drop will not be reported
either: name the schema elsewhere, or remove the table by hand.

An enum type is only dropped when a column uses it in the current schema and no declared
column still names it. A `TableDef` cannot describe an enum, so `push` from table definitions
never drops types that belong to something else.

## Generating migration files

For production, do not run `push`; generate SQL files, review them, and apply them in order.
The CLI works from a snapshot of the schema your code declares.

Emit one from the application:

```rust
let json = besu_sqlite::Database::schema_snapshot([users.def(), posts.def()])?;
std::fs::write("schema.json", json)?;
```

Then:

```console
$ besu generate --database sqlite:app.db --schema schema.json --out migrations
wrote 3 statement(s) to migrations/0001_schema.sql
```

`generate` introspects the database, diffs it against the snapshot, and writes the statements
through the matching DDL dialect. The filename is `{version:04}_{name}.sql`, where the version
is one more than the highest existing version in the directory and `name` comes from
`--name` (default `schema`, lowercased, spaces turned into underscores).

The generated file starts with a warning worth keeping:

```sql
-- Generated by `besu generate`.
-- Review before applying: dropping a column loses its data.
```

`generate` does not stop at `Risky` changes: it writes whatever the dialect's `change`
methods produce. On SQLite that is the table rebuild described above, which is why the
generated file can be long. It is still for review — a rebuild recreates the table, and any
trigger attached to it is not modelled (#66).

## The migration runner

A migration is a version, a name, and the statements that apply it — optionally with the
statements that undo it:

```rust
pub struct Migration {
    pub version: i64,
    pub name: String,
    pub statements: Vec<String>,
    pub down: Option<Vec<String>>,
}
```

`Migration::from_sql(version, name, sql)` splits the blob on `;`, respecting single-quoted
strings and `--` comments, and leaves `down` as `None`. `Migration::from_sql_pair(version,
name, up, down)` takes both halves; `down` is an `Option<String>` because either half may be
missing on its own.

`Migrator::from_dir` reads a directory of files:

```text
migrations/
  0001_create_users.sql
  0001_create_users.down.sql
  0002_add_email.sql
  0002_add_email.down.sql
```

`<version>_<name>.sql` is an up migration and `<version>_<name>.down.sql` is its down
migration. The two are paired by version — the identity the journal records — so a typo in
one name does not quietly lose the down half. A down file with no up file next to it is an
error, not a file that is silently ignored.

```rust
use besu::schema::Migrator;

let migrator = Migrator::from_dir("migrations")?;
let applied = db.migrate(&migrator).await?;
```

`Database::migrate` runs the whole set inside a transaction, returns the migrations that were
newly applied, and wraps any failure in `SchemaError::Migration`. It requires the driver to
implement `TransactionDriver`; the runner itself is generic over any `Executor`.

### The journal table

The runner keeps its history in a table it owns:

```sql
CREATE TABLE IF NOT EXISTS _besu_migrations (
    version BIGINT PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    checksum VARCHAR(64) NOT NULL,
    down_checksum VARCHAR(64)
)
```

On every run it:

1. Creates the journal if it does not exist.
2. Reads the applied versions, names and checksums.
3. Rejects two migrations with the same version.
4. Rejects an applied migration whose checksum no longer matches — up or down.
5. Executes the statements of each missing migration, then inserts its journal row.

A journal created before down migrations existed has three columns and no `down_checksum`.
There is no portable way to ask a table for its columns, so the runner probes for the fourth
one and adds it with `ALTER TABLE _besu_migrations ADD COLUMN down_checksum VARCHAR(64)` when
the probe fails. Nothing is lost; rows written before the upgrade simply have no down checksum
to compare against, which is why a down file can be added to an already-applied migration.

The checksum is an FNV-1a hash over the name and the statements, so editing a migration that
has already run is an error:

```text
migration 3 has been modified since it was applied (recorded checksum ..., now ...);
migrations are append-only
```

The down half is hashed separately, with its direction mixed in so the two can never collide,
and is recorded in the same row when the migration is applied. When a down checksum is present,
editing the `.down.sql` file is refused with the same message. The up checksum is deliberately
unchanged by this: adding a down file to an old migration does not invalidate its journal row.

The journal table is filtered out of introspection (`_besu_%`), so it never appears in a diff
or a generated migration.

### Down migrations

`Database::migrate_down` reverts applied migrations, one at a time, newest first.
`Database::migrate_down_one` is the single-step form, because only the journal knows which
version "one back" is:

```rust
// Revert the single most recent migration.
let reverted = db.migrate_down_one(&migrator).await?;

// Revert everything (0 means "no version left applied").
let reverted = db.migrate_down(&migrator, 0).await?;

// Revert until the newest applied version is at most 2.
let reverted = db.migrate_down(&migrator, 2).await?;
```

Both run inside a transaction, like `migrate`, so a failure part-way leaves every revert
undone. They return the reverted migrations, newest first. `Migrator::down_one` and
`Migrator::down_to` are the executor-level equivalents, for when you want to control the
transaction boundary yourself.

A revert:

1. Refuses if the newest applied migration has no down file — for example
   `migration 3 has no down file, so it cannot be reverted`. Nothing is skipped, because
   skipping would mean the journal and the schema disagree about which version is current.
2. Verifies both checksums, exactly as `migrate` does.
3. Runs the down statements **from the bottom of the file up**, then deletes the journal row.

The reverse order is what makes a down file readable: write it as the mirror of its up file —
the inverse of each statement in the same position — and the runner undoes them back to front.
An up file that adds a column and then indexes it gets a down file that drops the column and
then the index, and the runner drops the index before the column, which is the only order a
database accepts.

```sql
-- 0002_add_email.down.sql
ALTER TABLE users DROP COLUMN email;
DROP INDEX users_email_idx;
```

A migration with no down file is still perfectly valid; it just cannot be reverted.
`besu generate` writes up files only, so the down half is yours to write and review.

### All-or-nothing migrations

`Database::migrate` runs the whole migration set inside a transaction: it begins one, passes a
transaction executor to the runner, commits on success and rolls back on the first failure.
That gives atomicity *and* keeps every statement on the same connection, which matters on
SQLite where a pool may otherwise hand two statements of a schema change to different
connections.

It therefore requires `Dr: TransactionDriver`. For a driver without transactions, use
`Migrator::run` directly against whatever executor you have; statements then commit as they
run, so a failure can leave a migration half-applied:

```rust
let mut executor = &*db;
let applied = migrator.run(&mut executor).await?;
```

To control the boundary yourself, pass a transaction:

```rust
let mut tx = db.begin().await?;
let mut executor = &mut tx;
migrator.run(&mut executor).await?;
tx.commit().await?;
```

The error type changes to `MigrationError<Dr::Error>`, so handle it before committing.
`Migrator::run` takes `&mut E`, which is why the transaction is passed through a mutable
reference: `&mut Transaction` is the executor, not `Transaction` itself.

## CLI reference

The binary is `besu` (package `besu-cli`). All four commands take `--database`, which also
reads `DATABASE_URL`. The scheme selects the backend: `sqlite:`, `postgres:`/`postgresql:` or
`mysql:`.

```console
# Read the live schema into a snapshot (stdout by default).
besu pull --database postgres://user:pass@host/db --out schema.json

# Apply a snapshot. Destructive changes need --destructive.
besu push --database postgres://user:pass@host/db --schema schema.json
besu push --database postgres://user:pass@host/db --schema schema.json --dry-run

# Turn the difference into a numbered SQL file.
besu generate --database postgres://user:pass@host/db --schema schema.json \
    --out migrations --name add_email_index

# Apply pending files, in version order.
besu migrate --database postgres://user:pass@host/db --dir migrations

# Revert the newest one, or everything down to version 0.
besu migrate --database postgres://user:pass@host/db --dir migrations --down
besu migrate --database postgres://user:pass@host/db --dir migrations --down --to 0
```

| Command | Flags |
| --- | --- |
| `besu pull` | `--database <URL>`, `--out <FILE>` (optional) |
| `besu push` | `--database <URL>`, `--schema <FILE>`, `--destructive`, `--dry-run` |
| `besu generate` | `--database <URL>`, `--schema <FILE>`, `--out <DIR>` (default `migrations`), `--name <NAME>` (default `schema`) |
| `besu migrate` | `--database <URL>`, `--dir <DIR>` (default `migrations`), `--down`, `--to <VERSION>` |

`besu migrate --down` reverts instead of applying. On its own it reverts one migration;
`--to <VERSION>` reverts until the newest applied version is at most that version, so
`--to 0` reverts everything. `--to` requires `--down`; it is not a "migrate up to" flag.

`push --dry-run` prints one line per change, prefixed with its safety, and runs nothing:

```text
[safe] create table users
[risky] add column users.bio
[DESTRUCTIVE] drop column users.legacy
```

## What is checked

| Check | When |
| --- | --- |
| The declared schema and the live schema agree | Runtime, by `push` / `generate` |
| A destructive change needs `push_destructive` or `--destructive` | Runtime, before any statement |
| A dialect that cannot alter or rebuild a column refuses | Runtime, before any statement |
| A migration version is not duplicated | Runtime, when the runner starts |
| An applied migration was not edited | Runtime, by checksum, for both halves |
| A down file has an up file beside it | When the directory is read |
| The newest migration has a down file before it is reverted | Runtime, before any statement |
| A table is created after the tables it references | At DDL generation, via topological order |
| An enum type exists before a table that uses it, and is dropped after | At DDL generation, by the order of the changes |

## Gotchas

- **Renames are only detected when declared.** A rename looks exactly like a drop plus an
  add, so `diff` refuses to guess. Add `#[besu(renamed_from = "old_name")]` to the field and
  the migration becomes one `ALTER TABLE ... RENAME COLUMN`, which keeps the data. Without the
  attribute, `push` treats it as destructive and stops. Renaming a *table* has no such
  attribute yet.
- **`push` is not transactional.** Statements are executed one at a time. A failure halfway
  leaves the earlier ones applied. Use `generate` plus `migrate` for anything that matters.
- **Type widening is invisible on a dialect that folds types.** SQLite folds every integer
  width and `Char`/`VarChar` to their storage types; PostgreSQL folds unsigned integers to
  their signed storage. MySQL detects the change. Check `Ddl::normalise_type` for the
  dialect you are using.
- **SQLite rebuilds by dropping and recreating.** A type or nullability change recreates the
  table, so a trigger attached to it is lost (#66). Rows are copied, so the data survives.
- **`generate` writes statements, not judgements.** It emits whatever the dialect would do,
  warning where a change loses data. Read the file.
- **The CLI and `push` agree about types.** Both call `Ddl::normalise_type` for the target
  dialect, so `besu generate` and `besu push --dry-run` report exactly what `push` would do.
- **`Migrator::from_dir` silently skips up files it cannot parse.** A file whose name does not
  start with an integer is ignored; a typo in the version is easy to miss. A `.down.sql` file
  with no up file beside it is an error, because it can never run.
- **Reverting stops at the first migration without a down file.** It does not skip it: the
  newest applied version is the one that gets undone, and an un-revertable newest migration
  means nothing can go back. Add the down file, or revert in a different order by hand.
- **A down file is the mirror of its up file, and runs bottom up.** Write the inverse of each
  statement in the same position, and the runner executes them back to front. Writing the undo
  steps in the order you want them to run is the one mistake that makes a revert fail half way.
- **Down migrations are not generated.** `besu generate` writes only the up file, since the
  inverse of a schema change is not always recoverable (a dropped column has no data to put
  back). Write the `.down.sql` yourself and review it like any other migration.
- **Migrations are split on `;`.** `split_statements` understands quotes and `--` comments,
  but not PostgreSQL dollar-quoting, so a `DO $$ ... $$` block is split in the middle. Put
  such bodies in their own file with one statement, or run them outside the runner.
- **PostgreSQL cannot remove an enum value.** There is no `ALTER TYPE ... DROP VALUE`, so a
  schema that drops a value is reported as a `Destructive` `RemoveEnumValue` that renders no
  statements. Recreate the type (and rewrite every column that uses it) in a migration
  written by hand. Appending a value is a plain `ALTER TYPE ... ADD VALUE`.
- **An enum has to be declared to be managed.** `TableDef` has no spelling for an enum, so a
  schema built from table definitions carries no `enums` at all, and `push` leaves the
  database's types alone apart from dropping one that a column used to use. Use a snapshot
  (`besu pull` / `Schema::load`) when you want the diff to own the list.
- **SQLite cannot add a constraint to an existing table.** Adding or dropping a composite
  `UNIQUE` on SQLite reports the change and emits no statement; only a `CREATE TABLE` (or a
  table rebuild caused by an `AlterColumn`) writes one.
