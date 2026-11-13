# Demo fixtures

`schema.json`, `users.json`, `posts.json` and `comments.json` are fixtures generated from a
real SQLite database, so the hosted demo has a believable schema and rows to browse.
`source.ts` serves them through the same `DataSource` the Rust server implements; writes
mutate the in-memory copy (a reload restores it) and three canned read-only queries answer
in the console. Anything else refuses and points at a local `besu studio`.
