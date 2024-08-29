# Besu

A typesafe, async, and database-agnostic query builder for Rust.

Checkout the [documentation](https://docs.rs/besu) for more information.

## Features

 - Typesafe
 - Database-agnostic (support for [SQLite](https://sqlite.org), [PostgreSQL](https://www.postgresql.org), and [MySQL](https://www.mysql.com))
 - Schema-driven. No need for active DB to validate queries
 - Dynamic queries without loosing typesafety
 - Asynchronous (support for [Tokio](https://tokio.rs) and more)
 - Wasm compatible (coming soon)

## Inspiration

This project is heavily inspired by [Drizzle ORM](https://github.com/drizzle-team/drizzle-orm) and elements of [SQLx](https://github.com/launchbadge/sqlx) and [Prisma Client Rust](https://prisma.brendonovich.dev).