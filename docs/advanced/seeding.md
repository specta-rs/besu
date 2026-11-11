# Seeding

> **Not supported yet.** There is no `besu seed`, no seed file format, no factories and no
> deterministic random data. Seed rows are ordinary inserts, written by you.

Drizzle Seed generates rows from a schema and a set of factories. Besu has no equivalent
because there is nothing in the query layer to hang it on: a seed is a program that inserts
rows, and you already have the insert API. The two honest options are a small binary and a
migration.

## A seeding binary

A separate `main` is the flexible option: it can read fixtures, take a flag, run against any
environment and be thrown away afterwards.

```rust
use besu::prelude::*;

#[derive(Table)]
#[besu(table = "users")]
pub struct User {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    #[besu(unique)]
    pub email: String,
    pub name: String,
    pub age: i32,
    pub bio: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db = besu::Database::new(besu_sqlite::Sqlx::connect("sqlite://dev.db").await?);

    db.insert(users)
        .values([
            users.insert("oscar@example.com", "Oscar", 21),
            users.insert("alice@example.com", "Alice", 30),
        ])
        .on_conflict(users.email)
        .await?;

    Ok(())
}
```

`on_conflict(users.email)` renders `ON CONFLICT (email) DO NOTHING`, so running the program
twice does not duplicate the rows. Drop it if you want the second run to fail on the unique
constraint instead.

## A seed migration

When the rows are part of the deployed state — a lookup table, a default role — a migration
file is better: it runs once, in order, inside the migration runner's transaction, and the
journal records that it happened.

```sql
-- migrations/0003_seed_roles.sql
INSERT INTO roles (name) VALUES
    ('admin'),
    ('editor'),
    ('reader')
ON CONFLICT (name) DO NOTHING;
```

```text
migrations/
  0001_create_users.sql
  0002_create_roles.sql
  0003_seed_roles.sql
  0003_seed_roles.down.sql
```

The file is applied by `besu migrate`, or by `Migrator::from_dir` plus `db.migrate(..)` at
start-up. `ON CONFLICT` is PostgreSQL and SQLite spelling; MySQL needs `INSERT IGNORE` or an
explicit check. Writing the `.down.sql` is optional but expected: without it the migration
cannot be reverted.

## Defaults instead of seed values

A default belongs in the schema, not in a seed. A column with a default is not a required
insert argument, so every seed row omits it and the database supplies it.

```rust
#[derive(Table)]
#[besu(table = "users")]
pub struct User {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    #[besu(default = 0)]
    pub login_count: i32,
    #[besu(default_now)]
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[besu(default_sql = "gen_random_uuid()")]
    pub public_id: uuid::Uuid,
}
```

`default` accepts a string, integer, float or boolean literal; `default_sql` is inserted
verbatim and is the place for a database function; `default_now` renders
`DEFAULT CURRENT_TIMESTAMP`. The full list and the per-dialect rendering are in
[Declaring a schema](../schema/declaration.md).

## Gotchas

- **There is no environment guard.** A seeding binary pointed at production will seed
  production. Check the connection string yourself before it runs.
- **No factories, no RNG.** Fields are filled explicitly, or by a database default. That is
  the trade: a seed is obvious and boring, and it never silently changes shape.
- **A seed migration is forever.** It runs once per database, and editing an applied file is
  refused because the journal checksums it. Fix data with a new migration.
- **`values` is one statement and one round trip.** For a few thousand rows that is fine; for
  more, `values_chunked` or a `COPY`/`LOAD DATA` written with `sql!` is the practical route.
- **A seed row that references another row needs its id.** With `RETURNING`, take the id from
  the insert; without it (MySQL), select the row back by a unique column.
