#![allow(missing_docs, clippy::unwrap_used, clippy::panic, dead_code)]

//! Shared scaffolding for the integration tests.
//!
//! The suite runs against a real PostgreSQL server that every test binary shares, so each
//! test takes a database-wide advisory lock and drops everything the previous one left
//! behind before starting. The lock lives on one of the pool's connections, which means it
//! is released when the returned database is dropped, even if the test panics.

use besu::prelude::*;
#[cfg(feature = "native")]
use besu_postgres::Native;
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

/// Connect through the native `tokio-postgres` driver and wait for the suite-wide lock.
///
/// The lock lives on its own pool: a connection that a test detaches — dropping an
/// uncommitted transaction does exactly that — must not be the one holding the lock, or the
/// lock would be released while the test is still running. The returned tuple is the working
/// database and the lock-holding one, which the caller keeps alive.
#[cfg(feature = "native")]
pub async fn native_database() -> (besu::Database<Native>, besu::Database<Native>) {
    let lock = besu::Database::new(
        Native::new(URL)
            .await
            .expect("the test server must be reachable"),
    );
    lock.execute(sql!("SELECT pg_advisory_lock(4207)"))
        .await
        .expect("the suite lock must be available");

    let db = besu::Database::new(
        Native::new(URL)
            .await
            .expect("the test server must be reachable"),
    );

    (db, lock)
}

/// Remove every table, sequence and non-`public` schema, so each test starts from an empty
/// database.
///
/// Introspection reports every table outside PostgreSQL's system schemas, so anything a
/// previous test — or an interrupted run of it — left behind would show up as drift and
/// turn the next `push` destructive. The advisory lock guarantees nobody else is using the
/// database at this point.
///
/// This is generic over the driver so the sqlx and native suites share one reset.
pub async fn reset<Dr>(db: &besu::Database<Dr>)
where
    Dr: besu::Driver<Dialect = besu_postgres::Postgres>,
    String: besu::Decode<Dr>,
    bool: besu::Decode<Dr>,
{
    let tables: Vec<(String, String)> = db
        .execute(sql!(
            "SELECT t.schemaname, t.tablename FROM pg_tables t \
             WHERE t.schemaname NOT IN ('pg_catalog', 'information_schema') \
               AND t.schemaname NOT LIKE 'pg\\_%' ESCAPE '\\' \
               AND NOT EXISTS ( \
                   SELECT 1 FROM pg_depend d \
                   JOIN pg_class c ON c.oid = d.objid \
                   JOIN pg_namespace n ON n.oid = c.relnamespace \
                   WHERE d.deptype = 'e' \
                     AND n.nspname = t.schemaname AND c.relname = t.tablename)"
        ))
        .all()
        .await
        .expect("the table list must be readable");

    // A view does not depend on a table unless it selects from one, so dropping tables does
    // not remove a view over a constant or over a system catalogue. Drop views first, so a
    // leftover from an interrupted test cannot make the next introspection look destructive.
    let views: Vec<(String, String, bool)> = db
        .execute(sql!(
            "SELECT n.nspname, c.relname, c.relkind = 'm' FROM pg_class c \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.relkind IN ('v', 'm') \
               AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
               AND n.nspname NOT LIKE 'pg\\_%' ESCAPE '\\' \
               AND NOT EXISTS ( \
                   SELECT 1 FROM pg_depend d \
                   WHERE d.classid = 'pg_class'::regclass AND d.objid = c.oid \
                     AND d.deptype = 'e' \
               )"
        ))
        .all()
        .await
        .expect("the view list must be readable");

    for (schema, view, materialised) in views {
        let keyword = if materialised {
            "MATERIALIZED VIEW"
        } else {
            "VIEW"
        };
        let statement = format!(
            "DROP {keyword} IF EXISTS {}.{} CASCADE",
            quote_ident(&schema),
            quote_ident(&view)
        );
        db.execute(besu::Fragment::new(statement))
            .await
            .expect("a leftover view must be droppable");
    }

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

    // A standalone sequence outlives the table that used it unless it was `OWNED BY` one,
    // and a test that failed before its cleanup would leave it behind.
    let sequences: Vec<(String, String)> = db
        .execute(sql!(
            "SELECT s.schemaname, s.sequencename FROM pg_sequences s \
             WHERE s.schemaname NOT IN ('pg_catalog', 'information_schema') \
               AND s.schemaname NOT LIKE 'pg\\_%' ESCAPE '\\' \
               AND NOT EXISTS ( \
                   SELECT 1 FROM pg_depend d \
                   JOIN pg_class c ON c.oid = d.objid \
                   JOIN pg_namespace n ON n.oid = c.relnamespace \
                   WHERE d.deptype = 'e' \
                     AND n.nspname = s.schemaname AND c.relname = s.sequencename)"
        ))
        .all()
        .await
        .expect("the sequence list must be readable");
    for (schema, sequence) in sequences {
        let statement = format!(
            "DROP SEQUENCE IF EXISTS {}.{} CASCADE",
            quote_ident(&schema),
            quote_ident(&sequence)
        );
        db.execute(besu::Fragment::new(statement))
            .await
            .expect("a leftover sequence must be droppable");
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
    // A domain needs `DROP DOMAIN`; an enum needs `DROP TYPE`.
    let types: Vec<(String, String, String)> = db
        .execute(sql!(
            "SELECT n.nspname, t.typname, t.typtype::text FROM pg_type t \
             JOIN pg_namespace n ON n.oid = t.typnamespace \
             WHERE t.typtype IN ('e', 'd') \
               AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
               AND n.nspname NOT LIKE 'pg\\_%' ESCAPE '\\'"
        ))
        .all()
        .await
        .expect("the type list must be readable");

    for (schema, name, kind) in types {
        let keyword = if kind == "d" { "DOMAIN" } else { "TYPE" };
        let statement = format!(
            "DROP {keyword} IF EXISTS {}.{} CASCADE",
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
