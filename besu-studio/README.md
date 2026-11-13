# besu-studio

The server and frontend behind `besu studio`: a development-only database browser for Besu
databases. Browse and edit rows, filter and sort them, inspect the schema, and run SQL — from
one binary with nothing to install.

This crate is a library — the command lives in the CLI:

```console
$ besu studio --database postgres://user:password@localhost/app
besu studio 0.1.0 listening on http://127.0.0.1:4983
```

`--allow-remote` binds every interface; it is off by default because the studio runs arbitrary
SQL and has no authentication. Use it against a development database, never production.

The hosted copy at [studio.besu.specta.dev](https://studio.besu.specta.dev) is the same UI
without a server: it asks for the port a local `besu studio` is listening on and drives that.
Fixture data is there as a second option, for looking around without a database.

## Frontend

`web/` is a Vite + SolidJS + Tailwind application. The build is committed (`web/dist`) and
embedded into the binary by `build.rs`, so a Cargo build needs no Node toolchain:

```sh
pnpm --dir web install
pnpm --dir web dev     # http://localhost:5173, proxying /api to :4983
pnpm --dir web build   # rewrites web/dist, which is committed
```

`pnpm --dir web deploy` builds the bundle and publishes it to Cloudflare Workers.

## Documentation

- [The Besu guide](https://github.com/specta-rs/besu/tree/main/docs)
- [API reference](https://docs.rs/besu-studio)

## License

Licensed under the [MIT License](https://github.com/specta-rs/besu/blob/main/LICENSE).
