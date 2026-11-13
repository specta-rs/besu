# Besu studio — implementation contract

This is the shared spec for the redesigned studio. The frontend is Vite +
SolidJS + Tailwind v4; the Rust crate serves the built bundle and the JSON API
it reads.

Read this before touching code. File ownership is listed so parallel work does
not collide; do not edit files owned by another task.

## Layout

```
besu-studio/
  src/            Rust: axum router, API, embedded assets
  web/
    src/
      app.tsx              # shell glue (owned: integration)
      main.tsx             # entry
      styles.css           # design tokens + component classes
      lib/
        types.ts           # the API contract, in TypeScript
        api.ts             # HTTP client
        source.ts          # demo vs http switch
        store.tsx          # shared state (schema, toasts, theme)
        router.ts          # tiny history router
        ui.tsx             # Modal, Drawer, Spinner, EmptyState, Field
        format.ts          # type names, cell text, CSV, clipboard
      pages/               # one file per route
      components/shell/    # sidebar, topbar, palette, toasts
      components/...       # grid, console, schema
      demo/                # fixtures + in-memory DataSource
```

## Design language

Drizzle Studio is the reference, not a pixel target: a three-pane application
with a persistent object list, a dense data grid, and controls that stay out of
the way until hover. Concretely:

- 13px UI text, 12px monospace data, 28–32px controls. Dense, not airy.
- Dark by default. Tokens live in `src/styles.css`; never hardcode a colour.
  Use `bg-panel`, `border-line`, `text-ink-2`, `text-accent`, `bg-accent-soft`,
  and the `.btn`, `.field`, `.kbd`, `.panel` component classes.
- The grid is the product. Sticky header, hairline row separators, right-aligned
  numerics, `NULL` rendered as a dim italic `NULL`, booleans as a badge, JSON as
  a one-line summary. Selection is a tinted row, not a border.
- Every destructive action confirms. Every async action has a disabled state and
  a spinner. Empty tables get a real empty state, not a blank panel.
- Keyboard: `⌘K` palette, `⌘↵` run SQL, `Escape` closes overlays, `/` focuses
  the sidebar search when nothing else is focused.

## Data flow

`source` (`lib/source.ts`) is the only way to the backend. Never call `fetch`
from a component. `isDemo` is true for the Cloudflare build; in demo mode the
same interface is served from fixtures and writes mutate the in-memory copy.

Read state lives in `useStudio()`:

- `schema()` — `Resource<Schema | null>`; `schema()?.tables`, `.views`, `.enums`.
- `info()` — `Resource<ConnectionInfo | null>`; dialect + database label.
- `selectedTable()`, `selectTable(name)`.
- `revision()` — bumped after a successful write; include it in a
  `createEffect`/`createResource` dependency so tables reload.
- `refresh()`, `toast(message, kind)`, `theme()`, `toggleTheme()`,
  `paletteOpen()`, `setPaletteOpen(open)`.

Routing (`lib/router.ts`): `/` welcome, `/t/:table` data, `/schema/:table`
structure, `/sql` console. Use `navigate()` and `linkClick()`; never reload.

## API contract

All responses are JSON. Errors are `{ "error": string }` with a 4xx/5xx status.

### `GET /api/info`

```json
{ "dialect": "sqlite", "database": "app.db", "version": "0.1.0", "read_only": false }
```

### `GET /api/schema`

The `besu::schema::Schema` JSON as-is: `tables[]`, `enums[]`, `views[]`,
`sequences[]`. A `Table` carries `name`, optional `schema`, `columns[]`,
`indexes[]`, `uniques[]`, `foreign_keys[]`, `checks[]`. A `Column` carries
`name`, `ty` (Besu's canonical type: `"Bool"`, `{ "VarChar": 255 }`), `nullable`,
`auto_increment`, `primary_key`, `unique`, optional `default` (SQL text) and
`generated`.

### `GET /api/tables/{table}/rows`

Query: `limit` (default 50, max 500), `offset`, `sort=col:asc,other:desc`,
`filter` = JSON array of `{ "column": string, "op": FilterOp, "value"?: string }`
with `FilterOp` ∈ `eq neq gt gte lt lte contains starts_with is_null
is_not_null`.

```json
{
  "table": "users",
  "columns": [ { "name": "id", "ty": "Int8", "nullable": false, "primary_key": true } ],
  "rows": [ [1, "Oscar Beaumont", null] ],
  "total": 10,
  "limit": 50,
  "offset": 0,
  "primary_key": ["id"],
  "duration_ms": 3
}
```

`rows` are positional arrays in `columns` order. `total` is the count **after**
filters, so pagination is honest. `{table}` may be schema-qualified
(`app.users`, URL-encoded as `app.users`) and may name a view.

### `POST /api/tables/{table}/rows`

`{ "values": { "name": "Ada" } }` → `{ "inserted": 1 }`.

### `PATCH /api/tables/{table}/rows`

`{ "key": { "id": 7 }, "values": { "name": "Ada" } }` → `{ "updated": 1 }`. The
key must name every primary-key column; a table without a primary key refuses
with 400 rather than guessing.

### `DELETE /api/tables/{table}/rows`

`{ "key": { "id": 7 } }` → `{ "deleted": 1 }`. Same key rule.

### `POST /api/query`

`{ "sql": "select * from users" }` →

```json
{
  "columns": ["id", "name"],
  "rows": [[1, "Oscar"]],
  "rows_affected": 0,
  "last_insert_id": null,
  "duration_ms": 4
}
```

A statement that returns rows fills `columns`/`rows`; otherwise `rows_affected`
is the count. Exactly one statement is accepted. This works in demo mode only
for the fixtures' canned queries — the hosted demo refuses with a message that
says to run it locally.

## Task ownership

| Task | Owns | Must not touch |
| --- | --- | --- |
| API (Rust) | `src/api.rs`, `src/lib.rs`, `src/assets.rs`, `build.rs`, `Cargo.toml`, `tests/` | `web/src` |
| Shell | `web/src/components/shell/*`, `web/src/app.tsx` | `web/src/pages/*`, `web/src/lib/*` |
| Grid | `web/src/pages/data-page.tsx`, `web/src/components/grid/*` | `web/src/app.tsx`, `web/src/lib/*` |
| Console | `web/src/pages/sql-page.tsx`, `web/src/pages/schema-page.tsx`, `web/src/components/console/*`, `web/src/demo/*`, `wrangler.jsonc` | `web/src/app.tsx`, `web/src/lib/*` |

`web/src/lib/*` and `styles.css` are the shared contract: ask rather than
change them. If a shared helper is genuinely missing, add it to your own task's
files, not to `lib/`.
