# besu-bridge

A `Driver` for Besu that runs a query in a browser by sending it to a server.

The browser builds the same typed query it would build anywhere else and renders the exact
SQL; the bridge posts that SQL and its bound parameters to an HTTP endpoint, and the endpoint
runs them against a real database. The response carries the rows, so the browser decodes them
into the same structs a server would.

The server decides which database actually runs the statement, so the bridge is generic over
the dialect whose spelling it emits: `Bridge<FetchTransport, Postgres>` sends `$1` and quotes
identifiers the PostgreSQL way, while `Bridge<FetchTransport, MySql>` sends `?`.

Part of [Besu](https://github.com/specta-rs/besu), a typesafe, async, database-agnostic SQL
query builder for Rust.

## Documentation

- [Writing a driver](https://github.com/specta-rs/besu/blob/main/docs/drivers.md) — the wire
  protocol and both halves of the bridge
- [API reference](https://docs.rs/besu-bridge) — on docs.rs

## License

Licensed under the [MIT License](https://github.com/specta-rs/besu/blob/main/LICENSE).
