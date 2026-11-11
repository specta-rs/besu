#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

//! Shared scaffolding for the integration tests.
//!
//! The suite runs against a real PostgreSQL server that every test binary shares, so each
//! test takes a database-wide advisory lock and drops everything the previous one left
//! behind before starting. The lock lives on one of the pool's connections, which means it
//! is released when the returned database is dropped, even if the test panics.

use besu::prelude::*;
use besu_postgres::Sqlx;

/// The server the tests run against.
pub const URL: &str = "postgres://besu:besu@localhost:55432/besu";

/// Connect to the test server and wait until this test owns the suite-wide lock.
pub async fn database() -> besu::Database<Sqlx> {
    let db = besu::Database::new(
        Sqlx::new(URL)
            .await
            .expect("the test server must be reachable"),
    );

    // `execute` runs the statement and discards the row, leaving the lock held by whichever
    // pooled connection served it.
    db.execute(sql!("SELECT pg_advisory_lock(4207)"))
        .await
        .expect("the suite lock must be available");

    db
}

/// Remove every table and every non-`public` schema, so each test starts from an empty
/// database.
///
/// Introspection reports every table outside PostgreSQL's system schemas, so anything a
/// previous test — or an interrupted run of it — left behind would show up as drift and
/// turn the next `push` destructive. The advisory lock guarantees nobody else is using the
/// database at this point.
pub async fn reset(db: &besu::Database<Sqlx>) {
    let tables: Vec<(String, String)> = db
        .execute(sql!(
            "SELECT schemaname, tablename FROM pg_tables \
             WHERE schemaname NOT IN ('pg_catalog', 'information_schema') \
               AND schemaname NOT LIKE 'pg\\_%' ESCAPE '\\'"
        ))
        .all()
        .await
        .expect("the table list must be readable");

    for (schema, table) in tables {
        // Identifiers cannot be bound as parameters, so they are quoted by hand.
        let statement = format!(
            "DROP TABLE IF EXISTS {}.{} CASCADE",
            quote_ident(&schema),
            quote_ident(&table)
        );
        db.execute(besu::Fragment::new(statement))
            .await
            .expect("a leftover table must be droppable");
    }

    let schemas: Vec<String> = db
        .execute(sql!(
            "SELECT nspname FROM pg_namespace \
             WHERE nspname NOT IN ('pg_catalog', 'information_schema', 'public') \
               AND nspname NOT LIKE 'pg\\_%' ESCAPE '\\'"
        ))
        .all()
        .await
        .expect("the schema list must be readable");

    for schema in schemas {
        let statement = format!("DROP SCHEMA {} CASCADE", quote_ident(&schema));
        db.execute(besu::Fragment::new(statement))
            .await
            .expect("a leftover schema must be droppable");
    }

    // A type outlives the table that used it, so an interrupted run leaves it behind too.
    let types: Vec<(String, String)> = db
        .execute(sql!(
            "SELECT n.nspname, t.typname FROM pg_type t \
             JOIN pg_namespace n ON n.oid = t.typnamespace \
             WHERE t.typtype = 'e' \
               AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
               AND n.nspname NOT LIKE 'pg\\_%' ESCAPE '\\'"
        ))
        .all()
        .await
        .expect("the type list must be readable");

    for (schema, name) in types {
        let statement = format!(
            "DROP TYPE IF EXISTS {}.{} CASCADE",
            quote_ident(&schema),
            quote_ident(&name)
        );
        db.execute(besu::Fragment::new(statement))
            .await
            .expect("a leftover type must be droppable");
    }
}

/// Quote an identifier for the statements that cannot take a bound parameter.
fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}
