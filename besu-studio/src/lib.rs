//! A development-only schema and data browser for Besu databases.
//!
//! The studio is the visual counterpart to `besu-cli`: it introspects a live database,
//! serves its schema and a paged view of every table, and runs SQL from a single page. It is
//! modelled on Drizzle Studio, but it is deliberately smaller.
//!
//! <section class="warning">
//!
//! The query endpoint runs arbitrary SQL. There is no authentication, no read-only mode and
//! no audit trail, so this crate refuses to bind anything except the loopback interface
//! unless the caller opts in, and the startup message says so plainly. Treat every
//! `besu-studio` process as a shell with a database connection.
//!
//! </section>
//!
//! The router is built by [`router`], which takes a connected [`AnyDatabase`]. That function
//! is the whole library: a test can build it without opening a socket.
#![forbid(unsafe_code)]

mod api;

use axum::{Router, routing::get};
use besu_any::AnyDatabase;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::Level;

/// Serve the studio until the listener stops.
///
/// This exists so the binary that runs the studio does not need `axum` as a dependency: the
/// CLI depends on this crate, and the HTTP stack stays inside it.
pub async fn serve(listener: tokio::net::TcpListener, db: AnyDatabase) -> std::io::Result<()> {
    axum::serve(listener, router(db)).await
}

/// Build the studio's router around a live database.
///
/// The returned router serves:
///
/// - `GET /` — the single-page UI,
/// - `GET /api/schema` — the introspected [`Schema`](besu::schema::Schema) as JSON,
/// - `GET /api/tables/{table}/rows?limit=50&offset=0` — a page of rows,
/// - `POST /api/query` — one SQL statement, executed against the database.
///
/// `db` must outlive the router logically rather than by reference: the value is cloned into
/// the router's state, and every clone shares the underlying connection pool.
pub fn router(db: AnyDatabase) -> Router {
    Router::new()
        .route("/", get(api::index))
        .route("/api/schema", get(api::schema))
        .route("/api/tables/{table}/rows", get(api::table_rows))
        .route("/api/query", axum::routing::post(api::query))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .with_state(db)
}
