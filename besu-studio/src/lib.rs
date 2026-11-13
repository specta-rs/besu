//! A development-only schema and data browser for Besu databases.
//!
//! The studio is the visual counterpart to `besu-cli`: it introspects a live database,
//! serves its schema, a paged and filterable view of every table, an inline editor and a
//! SQL console. The UI is a SolidJS application built by Vite and embedded into this
//! crate, so `besu studio` is still a single binary with nothing to install.
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
mod assets;

use axum::{
    Router,
    extract::Request,
    routing::{get, post},
};
use besu_any::AnyDatabase;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::Level;

/// What the UI shows about the connection it is browsing.
#[derive(Debug, Clone)]
pub struct StudioInfo {
    /// The connection string, with any password removed.
    pub connection: String,
    /// The dialect's name: `sqlite`, `postgres` or `mysql`.
    pub dialect: &'static str,
    /// The crate version, so the footer can name the build.
    pub version: &'static str,
}

/// Everything a request handler needs.
#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) db: AnyDatabase,
    pub(crate) info: StudioInfo,
}

/// Serve the studio until the listener stops.
///
/// This exists so the binary that runs the studio does not need `axum` as a dependency: the
/// CLI depends on this crate, and the HTTP stack stays inside it. `connection` is the URL
/// the CLI was given; it is shown in the UI with the password stripped.
pub async fn serve(
    listener: tokio::net::TcpListener,
    db: AnyDatabase,
    connection: impl Into<String>,
) -> std::io::Result<()> {
    axum::serve(listener, router(db, connection)).await
}

/// Build the studio's router around a live database.
///
/// The returned router serves the embedded frontend and the JSON API:
///
/// - `GET /api/info` — dialect, database and version,
/// - `GET /api/schema` — the introspected [`Schema`](besu::schema::Schema) as JSON,
/// - `GET /api/tables/{table}/rows?limit=&offset=&sort=&filter=` — a page of rows,
/// - `POST|PATCH|DELETE /api/tables/{table}/rows` — insert, update and delete one row,
/// - `POST /api/query` — one SQL statement, executed against the database,
/// - everything else — the single-page application.
///
/// `db` must outlive the router logically rather than by reference: the value is cloned into
/// the router's state, and every clone shares the underlying connection pool.
pub fn router(db: AnyDatabase, connection: impl Into<String>) -> Router {
    let dialect = match &db {
        AnyDatabase::Sqlite(_) => "sqlite",
        AnyDatabase::Postgres(_) => "postgres",
        AnyDatabase::MySql(_) => "mysql",
    };

    let state = AppState {
        db,
        info: StudioInfo {
            connection: redact(&connection.into()),
            dialect,
            version: env!("CARGO_PKG_VERSION"),
        },
    };

    Router::new()
        .route("/api/info", get(api::info))
        .route("/api/schema", get(api::schema))
        .route(
            "/api/tables/{table}/rows",
            get(api::table_rows)
                .post(api::insert_row)
                .patch(api::update_row)
                .delete(api::delete_row),
        )
        .route("/api/query", post(api::query))
        .fallback(fallback)
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .with_state(state)
}

/// Serve an asset, or the app shell for a client-side route.
async fn fallback(request: Request) -> axum::response::Response {
    assets::asset(request.uri().clone())
}

/// The connection string without its password.
///
/// A URL reaches the UI only to be read, so `postgres://user:secret@host/db` is shown as
/// `postgres://user@host/db`; a scheme-less DSN (`user=… password=…`) has its password
/// parameter blanked instead.
fn redact(connection: &str) -> String {
    let Some((scheme, rest)) = connection.split_once("://") else {
        return connection
            .split_whitespace()
            .map(|part| {
                if part.to_ascii_lowercase().starts_with("password=") {
                    "password=***".to_owned()
                } else {
                    part.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
    };

    let (authority, tail) = rest.split_once('/').unwrap_or((rest, ""));

    let authority = match authority.rsplit_once('@') {
        Some((user, host)) => match user.split_once(':') {
            Some((user, _)) => format!("{user}@{host}"),
            // No password to remove.
            None => authority.to_owned(),
        },
        None => authority.to_owned(),
    };

    if tail.is_empty() {
        format!("{scheme}://{authority}")
    } else {
        format!("{scheme}://{authority}/{tail}")
    }
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn a_password_never_reaches_the_ui() {
        assert_eq!(
            redact("postgres://besu:hunter2@localhost:5432/app"),
            "postgres://besu@localhost:5432/app"
        );
        assert_eq!(
            redact("mysql://root:secret@127.0.0.1/db?ssl=true"),
            "mysql://root@127.0.0.1/db?ssl=true"
        );
        assert_eq!(redact("sqlite:///tmp/app.db"), "sqlite:///tmp/app.db");
        assert_eq!(redact("sqlite::memory:"), "sqlite::memory:");
        assert_eq!(
            redact("host=localhost user=besu password=hunter2 dbname=app"),
            "host=localhost user=besu password=*** dbname=app"
        );
    }
}
