# Generated columns

> **Not supported yet.** `#[derive(Table)]` has no attribute for a generated column, and the
> schema IR has no field that describes one. Create it in a migration and read it with
> `sql!`.

A generated column is computed by the database from an expression over other columns. It is
either `STORED`, meaning the value is written on each insert and update, or `VIRTUAL`, meaning
it is computed when read. The typed builder has no column handle for one, so the model should
not declare it.

## Creating one

Write the `ALTER TABLE` in a migration; see
[Migrations](../migrations/overview.md) for the file layout. Each dialect has its own form,
and MySQL is the one that needs an explicit type and `CONCAT`.

```sql
-- SQLite
ALTER TABLE "users"
    ADD COLUMN "full_name" TEXT
    GENERATED ALWAYS AS ("first_name" || ' ' || "last_name") STORED;
```

```sql
-- PostgreSQL
ALTER TABLE "users"
    ADD COLUMN "full_name" TEXT
    GENERATED ALWAYS AS ("first_name" || ' ' || "last_name") STORED;
```

```sql
-- MySQL: a generated column needs an explicit type, and CONCAT rather than ||
ALTER TABLE `users`
    ADD COLUMN `full_name` VARCHAR(255)
    GENERATED ALWAYS AS (CONCAT(`first_name`, ' ', `last_name`)) STORED;
```

## Reading one

Keep the column out of the struct. `db.select(users)` projects the declared columns only, so
the generated one has to be named in raw SQL, with an explicit column list and an explicit row
type:

```rust
let rows: Vec<(i64, String)> = db
    .execute(sql!("SELECT id, full_name FROM users ORDER BY id"))
    .all()
    .await?;
```

`pick` cannot help: it takes a column handle or an expression built from one, and there is no
handle for a column the derive never saw. `SELECT *` does not help either: a model decodes by
position, so the extra column is not read into anything.

Do not work around this by declaring `full_name` as an ordinary field. The insert struct would
require it as an argument, so the generated `INSERT` would name it, and the database rejects a
write to a generated column. The DDL would also disagree with the table, which is what the next
section is about.

## What `push` does

The derive is not the only layer that misses the column; introspection does too, but not
uniformly.

| Database | Introspection | What `push` does |
| --- | --- | --- |
| SQLite | `PRAGMA table_info` lists ordinary columns only, so the generated column is absent | Nothing. It is invisible until a table rebuild recreates the table from the IR |
| PostgreSQL | `information_schema.columns` includes it, and the query does not filter `is_generated` | Sees a column the derive did not declare and proposes `DROP COLUMN` |
| MySQL | `information_schema.COLUMNS` includes it; `EXTRA` is only inspected for `auto_increment` and expression defaults | The same: it proposes `DROP COLUMN` |

`Change::DropColumn` is destructive, so plain `push` refuses with `SchemaError::Destructive`
and `push_destructive` applies it. On PostgreSQL and MySQL a snapshot from `besu pull` records
the column as an ordinary one, so `push --schema` against that snapshot leaves it alone; that
does not make `create_schema` understand it.

On SQLite there is a sharper edge. Because the column is absent from the IR, a table rebuild
— which `push` performs to change any other column — creates the table from the declared
columns and copies the shared ones across. The generated column is not in either list, so it
is gone after the rebuild. Keep schema changes to that table in migrations.

## What is checked

| Mistake | When it fails |
| --- | --- |
| Declaring a generated column in the model | Not a compile error; `push` disagrees with the table, and a required insert argument cannot be written |
| Referencing the column from `pick` or `filter` | Compile error: the derive generated no handle |
| Expecting `SELECT *` to return the generated column | No error; the extra column is ignored by the model |
| `push` on PostgreSQL or MySQL | Runtime, `SchemaError::Destructive`, unless forced |
| A SQLite table rebuild after the column was added | No error; the column is silently lost |

## Gotchas

- **`push` is not the tool for this table.** A migration is the only place the column can be
  created, so it is also the place a change to it belongs.
- **`STORED` versus `VIRTUAL` is a migration decision.** A stored column costs space on every
  row and is recomputed on write; a virtual one costs time on every read. PostgreSQL can index
  only the stored form; MySQL and SQLite can index both.
- **The expression has dialect-specific differences.** `||` concatenates on SQLite and
  PostgreSQL but is a logical OR on MySQL, which needs `CONCAT`. Functions differ further;
  `sql!` cannot check any of it.
- **A generated column is not selected by the default projection.** A migration cannot change
  that, because the projection comes from the `TableDef`.
