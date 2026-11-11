#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

//! Shared scaffolding for the integration tests.
//!
//! The suite runs against a real PostgreSQL server that every test binary shares, so each
//! test takes a database-wide advisory lock and drops the tables it knows about before
//! starting. The lock lives on one of the pool's connections, which means it is released
//! when the returned database is dropped, even if the test panics.

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

/// Remove every table the suite creates, so each test starts from a known schema.
pub async fn reset(db: &besu::Database<Sqlx>) {
    db.execute(sql!(
        "DROP TABLE IF EXISTS posts, users, things, numbers, null_probe, \"_besu_migrations\" CASCADE"
    ))
    .await
    .expect("the test tables must be droppable");
}
