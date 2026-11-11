# Row-level security

PostgreSQL row-level security is a policy attached to a table. Besu has no IR type for a
policy, so `push` neither creates nor drops one: it is written in a migration and reviewed
like any other hand-written SQL.

> **Not supported yet.** A policy has no Rust declaration and no diff. See the
> [roadmap](../roadmap.md) and [Drizzle parity](../parity.md).

```sql
-- migrations/0007_tenant_isolation.sql
ALTER TABLE documents ENABLE ROW LEVEL SECURITY;

CREATE POLICY documents_tenant ON documents
    USING (tenant_id = current_setting('app.tenant')::bigint);
```

```sql
-- migrations/0007_tenant_isolation.down.sql
DROP POLICY documents_tenant ON documents;
ALTER TABLE documents DISABLE ROW LEVEL SECURITY;
```

A policy applies to a role or to `PUBLIC`, and `FORCE ROW LEVEL SECURITY` makes it apply to
the table's owner too. Both are ordinary SQL inside the same migration.

## Why the differ cannot see a policy

Introspection never reads `pg_policy`, so a policy is invisible to `push`. That is deliberate:
if it were reported, the differ would see something the declared schema cannot express, and
`push` would try to drop or replace it. Leaving it invisible means `push` will not remove it,
and also cannot create it.

The policy is still attached to the table, so a `DROP TABLE` in a migration removes it, and a
`DROP COLUMN` it names fails until the policy is dropped first.

## Gotchas

- **Run migrations as the right role.** A superuser, or a role with `BYPASSRLS`, ignores
  policies, so a migration that tests them can appear to pass while a normal role is denied.
- **`ENABLE` is not `FORCE`.** The table owner bypasses policies unless `FORCE ROW LEVEL
  SECURITY` is set.
- **There is no `CREATE POLICY IF NOT EXISTS`.** Re-running a policy migration by hand fails;
  the journal is what stops it running twice.
- **A view over a protected table runs as its owner by default.** On PostgreSQL 15 and later,
  set `security_invoker = true` on the view; before that, filter inside the view.
