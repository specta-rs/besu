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
    pub foreign_keys: Vec<ForeignKey>,
    pub comment: Option<String>,
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

Ordering is deterministic on purpose. `Schema::sort()` sorts tables by name and normalises
each table (primary-key columns first, indexes and foreign keys sorted by name), and
`topological_order()` returns tables after the tables they reference. That means two
equivalent schemas produce the same DDL and the same JSON, which is what makes snapshots
reviewable.

### Normalisation matters

`diff` compares types literally. `diff_with(current, desired, normalise)` takes a function and
is what `push` uses, passing `Ddl::normalise_type` from the dialect. A dialect whose storage
collapses several canonical types therefore supplies its own folding:

| Dialect | Folds |
| --- | --- |
| SQLite | `Int2`/`Int4`/`Int8` and every `UInt` → `Int8`; `Char`/`VarChar` → `Text`; `Jsonb` → `Json`; `TimestampTz` → `Timestamp` |
| PostgreSQL | `UInt(8)` → `Int2`; `UInt(16)` → `Int4`; `UInt(32)` → `Int8`; `UInt(64)` → `Decimal(20, 0)` |
| MySQL | Nothing — it stores the declared types faithfully |

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
| `AddColumn` (nullable or with a default) | Safe | Existing rows get a value |
| `AddColumn` (`NOT NULL`, no default) | Risky | Fails on a non-empty table |
| `AddIndex` | Risky | Rewrites data, but is reversible |
| `AddForeignKey` | Risky | Validates existing rows |
| `AlterColumn` (same type/nullable/default/auto-increment) | Safe | Metadata-only |
| `AlterColumn` (anything else) | Risky | Rewrites the column |
| `DropForeignKey` | Risky | Loses a constraint, not data |
| `DropColumn` | Destructive | Loses data |
| `DropIndex` | Destructive | Loses an index (and possibly uniqueness) |
| `DropTable` | Destructive | Loses data |

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
place. SQLite cannot (`can_alter_column() == false`), so a schema change that needs an
`ALTER COLUMN` is refused with a clear error rather than the default drop-and-add:

```text
unsupported: sqlite cannot change a column in place; write an explicit migration instead
```

That is deliberate. The portable default implementation of `alter_column` is a `DROP COLUMN`
followed by an `ADD COLUMN`, which loses the column's data; silently doing that during `push`
would be worse than stopping.

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

`generate` does not stop at `Risky` changes and does not refuse an alter the database cannot
do: it writes whatever the dialect's `change` methods produce. On SQLite, an `AlterColumn`
therefore becomes the default drop-and-add pair, with a warning at the top of the file. That is
exactly why the file is for review.

## The migration runner

A migration is a version, a name and one or more statements:

```rust
pub struct Migration {
    pub version: i64,
    pub name: String,
    pub statements: Vec<String>,
}
```

`Migration::from_sql(version, name, sql)` splits the blob on `;`, respecting single-quoted
strings and `--` comments. `Migrator::from_dir` reads `<version>_<name>.sql` files from a
directory and sorts them by version.

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
    checksum VARCHAR(64) NOT NULL
)
```

On every run it:

1. Creates the journal if it does not exist.
2. Reads the applied versions, names and checksums.
3. Rejects two migrations with the same version.
4. Rejects an applied migration whose checksum no longer matches.
5. Executes the statements of each missing migration, then inserts its journal row.

The checksum is an FNV-1a hash over the name and the statements, so editing a migration that
has already run is an error:

```text
migration 3 has been modified since it was applied (recorded checksum ..., now ...);
migrations are append-only
```

The journal table is filtered out of introspection (`_besu_%`), so it never appears in a diff
or a generated migration.

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
```

| Command | Flags |
| --- | --- |
| `besu pull` | `--database <URL>`, `--out <FILE>` (optional) |
| `besu push` | `--database <URL>`, `--schema <FILE>`, `--destructive`, `--dry-run` |
| `besu generate` | `--database <URL>`, `--schema <FILE>`, `--out <DIR>` (default `migrations`), `--name <NAME>` (default `schema`) |
| `besu migrate` | `--database <URL>`, `--dir <DIR>` (default `migrations`) |

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
| SQLite cannot alter a column in place | Runtime, before any statement |
| A migration version is not duplicated | Runtime, when the runner starts |
| An applied migration was not edited | Runtime, by checksum |
| A table is created after the tables it references | At DDL generation, via topological order |

## Gotchas

- **Renames are drop plus add.** `diff` deliberately refuses to guess. Renaming a column or a
  table looks exactly like dropping one and creating another, so `push` treats it as
  destructive and `generate` writes a `DROP`/`ADD` pair. Write the rename by hand.
- **`push` is not transactional.** Statements are executed one at a time. A failure halfway
  leaves the earlier ones applied. Use `generate` plus `migrate` for anything that matters.
- **Type widening is invisible on a dialect that folds types.** SQLite folds every integer
  width and `Char`/`VarChar` to their storage types; PostgreSQL folds unsigned integers to
  their signed storage. MySQL detects the change. Check `Ddl::normalise_type` for the
  dialect you are using.
- **SQLite cannot alter.** A type or nullability change needs a table rebuild, written as an
  explicit migration.
- **`generate` writes a warning, not a refusal.** On a database that cannot alter a column in
  place (SQLite), an `AlterColumn` becomes a drop-and-add. The generated file starts with a
  warning saying the column's data will be lost, and then writes the statements anyway. Read
  the file.
- **The CLI's diff is the identity diff.** `besu generate` and `besu push --dry-run` call
  `diff`, while `Database::push` calls `diff_with` with `Ddl::normalise_type`. On a dialect
  that folds types (SQLite, or PostgreSQL's unsigned integers) the CLI can therefore report a
  type change that `push` itself would consider equal. Read the generated file; the fix is to
  route the CLI through `diff_with` too.
- **`Migrator::from_dir` silently skips files it cannot parse.** A file whose name does not
  start with an integer is ignored; a typo in the version is easy to miss.
- **Migrations are split on `;`.** `split_statements` understands quotes and `--` comments,
  but not PostgreSQL dollar-quoting, so a `DO $$ ... $$` block is split in the middle. Put
  such bodies in their own file with one statement, or run them outside the runner.
- **Enums are stored but not generated.** `Schema::enums` exists for round-tripping a
  PostgreSQL database, but the DDL layer has no `CREATE TYPE` step and the diff ignores them.
