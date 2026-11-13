# `besu studio`

A database browser served by the CLI: browse and edit rows, filter and sort them, read the
schema, and run SQL. It introspects the database it is pointed at, so there is no config file
and nothing else to install.

```sh
besu studio --database sqlite://app.db
# besu studio 0.1.0 listening on http://127.0.0.1:4983
```

```sh
Usage: besu studio [OPTIONS] --database <DATABASE>

Options:
      --database <DATABASE>
          Connection string for the database to browse, such as `postgres://…` or `sqlite://app.db`

      --port <PORT>
          Port to listen on

          [default: 4983]

      --allow-remote
          Bind every network interface instead of loopback.

          This makes the studio reachable by anyone who can reach the machine, and the studio has no authentication.

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version
```

`--database` reads `DATABASE_URL` like every other subcommand, so a shell that already exports
it needs no flag. `RUST_LOG` is honoured — `besu studio` is the one subcommand that installs a
tracing subscriber, because a server without its request log is hard to trust.

## The interface

Three panes, one page:

- **The sidebar** lists tables and views, searchable, with the connection it is browsing. It
  keeps the SQL console one click away.
- **The data view** is a grid over one table: pagination, a filter bar, multi-column sorting,
  column visibility, CSV and JSON export of the page, and a sticky primary-key column so a
  wide table stays readable.
- **The structure view** is the same table as the database sees it: columns and types, the
  primary key, uniques, indexes, foreign keys and checks, plus the `CREATE TABLE` the IR
  renders for copy-paste.

Clicking a row opens an editor with a control per column type — a checkbox for a boolean, a
textarea for JSON, a NULL toggle for a nullable column. Only changed fields are written.
Adding a row uses the same controls and leaves defaults to the database. Deleting asks first,
and names the primary key it is about to remove.

The **SQL console** runs one statement at a time and shows the result set with the same
density as the grid, along with the row count and how long it took. The last twenty statements
are kept for recall; in a browser, they survive navigation.

### Keyboard

| Keys | Action |
| --- | --- |
| `⌘K` / `Ctrl K` | Command palette: any table, view or action |
| `/` | Focus the table search |
| `↑` `↓` or `j` `k` | Move the row selection |
| `Enter` | Open the selected row |
| `Escape` | Close the drawer, dialog or palette |
| `⌘↵` / `Ctrl ↵` | Run the statement (SQL console) |
| `↑` `↓` | Recall a previous statement (SQL console) |

## The hosted frontend

[studio.besu.specta.dev](https://studio.besu.specta.dev) serves the same frontend without a
database behind it. It asks for the port `besu studio` is running on, probes
`http://127.0.0.1:<port>/api/info`, and then drives that local server directly — the hosted page
is only the UI, and every row, write and query reaches the database through the process on your
machine.

The server allows that origin (and the Vite dev server) in its CORS layer, and answers Chrome's
private-network preflight, which is what lets an HTTPS page reach loopback at all. Nothing else
is allowed: a different site cannot read this studio, and the server is still loopback-only.

If no server answers, the page says so and shows the command to start one. "Explore with demo
data" is a quiet second option: three fixture tables in memory, where browsing, filtering and
the editors work, writes never leave the page, and the SQL console explains that it needs a
local process.

## How it ships

The frontend is Vite + SolidJS + Tailwind in `besu-studio/web`. It is built once to
`besu-studio/web/dist` and committed; `besu-studio/build.rs` embeds that directory into the
binary, so `cargo build` needs no Node toolchain, and the same files are uploaded to Cloudflare
for the hosted copy. The server answers:

| Route | Purpose |
| --- | --- |
| `GET /api/info` | Dialect, database and version |
| `GET /api/schema` | The introspected `Schema` as JSON |
| `GET /api/tables/{table}/rows` | A page of rows, `?limit=`, `?offset=`, `?sort=`, `?filter=` |
| `POST` `PATCH` `DELETE /api/tables/{table}/rows` | Insert, update or delete one row |
| `POST /api/query` | One SQL statement, with its result set |

`{table}` may be schema-qualified and may name a view. Row pages carry `columns` with their
Besu types, so the client renders and edits each value the way its type deserves.

## Security posture

There is no authentication, no read-only mode and no audit trail. The query box runs arbitrary
SQL, including statements that write. Treat every process as a shell with a database
connection.

The server therefore:

- binds `127.0.0.1` unless `--allow-remote` is passed, and prints what that means when it is;
- refuses to serve a table name that is not in the introspected schema;
- builds every statement through `Sql`, which quotes identifiers the dialect's own way;
- strips the password out of the connection string before the UI ever sees it;
- splits the query body with the same splitter the migration runner uses, and refuses more
  than one statement.

It has no notion of users or permissions: there is no per-user access control, and no query
timeout. PostgreSQL row-level security still applies to whatever role the URL connects as,
and the studio does nothing to change that.

## Gotchas

- **`--allow-remote` is an explicit exposure.** Anyone who can reach the port can read and
  modify the database.
- **Editing needs a primary key.** Without one, a row cannot be identified, so the drawer
  shows the values read-only and refuses to save or delete rather than guessing.
- **A row page is capped at 500 rows.** Use the SQL console for anything larger; its result
  set is capped too.
- **The SQL console returns text.** A statement's result columns are read through a cast, so
  every cell arrives as a string and the console does not restore numbers or booleans the way
  the table grid does.
- **`?` is a placeholder.** A bare `?` in the SQL box has no value, so the statement fails at
  the server; `??` is a literal question mark.
- **It is not a migration tool.** It never diffs and never applies a schema; use
  [`besu push`](push.md) or [`besu migrate`](migrate.md).

See [the architecture](../architecture.md#the-studio) for how the studio uses the dynamic
APIs.
