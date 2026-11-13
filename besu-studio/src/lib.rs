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
    http::{HeaderValue, Method, header},
    middleware::Next,
    response::Response,
    routing::{get, post},
};
use besu_any::AnyDatabase;
use std::time::Duration;
use tower_http::cors::{AllowOrigin, CorsLayer};
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
/// The API answers CORS preflights from `https://studio.besu.specta.dev`, the
/// hosted copy of the frontend, so that page can drive a studio running on the
/// same machine. No other origin is allowed.
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
        .layer(cors())
        .layer(axum::middleware::from_fn(allow_private_network))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .with_state(state)
}

/// The origins allowed to read this server from a browser.
///
/// Two kinds qualify. The hosted frontend is a public page that connects to a
/// local studio, so it is named exactly. Everything else has to look like a
/// development origin itself — loopback, a private network address, or a name
/// that only resolves locally — which is what lets a studio served from one
/// machine drive a studio on another, without allowing an arbitrary website to
/// read a database sitting on loopback.
///
/// `Vary: Origin` comes with the layer, so a cache cannot serve one origin's
/// headers to another.
fn cors() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin, _| {
            origin == "https://studio.besu.specta.dev" || is_local_origin(origin)
        }))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::CONTENT_TYPE, header::ACCEPT])
        .max_age(Duration::from_secs(600))
}

/// Whether an `Origin` header names something only reachable on a local or
/// private network.
///
/// The comparison is textual for names and numeric for addresses: `localhost`,
/// `*.local`, `*.ts.net` (Tailscale's MagicDNS), `127.0.0.0/8`, `10.0.0.0/8`,
/// `172.16.0.0/12`, `192.168.0.0/16`, `169.254.0.0/16`, `100.64.0.0/10` and
/// IPv6 loopback or unique-local addresses.
fn is_local_origin(origin: &HeaderValue) -> bool {
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let Some((scheme, rest)) = origin.split_once("://") else {
        return false;
    };
    if scheme != "http" && scheme != "https" {
        return false;
    }

    // An IPv6 literal keeps its brackets, and its own colons, out of the port.
    let host = if let Some(stripped) = rest.strip_prefix('[') {
        match stripped.split_once(']') {
            Some((address, _)) => format!("[{address}]"),
            None => return false,
        }
    } else {
        rest.split(':').next().unwrap_or(rest).to_owned()
    };

    if matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]") {
        return true;
    }
    if host.ends_with(".local") || host.ends_with(".ts.net") {
        return true;
    }

    match host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<std::net::IpAddr>()
    {
        Ok(std::net::IpAddr::V4(address)) => {
            address.is_loopback() || address.is_private() || address.is_link_local() || {
                // Tailscale hands out addresses from 100.64.0.0/10, which the
                // standard library calls shared rather than private.
                let octets = address.octets();
                octets[0] == 100 && (64..128).contains(&octets[1])
            }
        }
        Ok(std::net::IpAddr::V6(address)) => {
            address.is_loopback() || (address.segments()[0] & 0xfe00) == 0xfc00
        }
        Err(_) => false,
    }
}

/// Answer Chrome's private-network preflight.
///
/// A page served over HTTPS may only reach `127.0.0.1` after a preflight that
/// asks for permission with `Access-Control-Request-Private-Network`; the
/// answer has to be explicit or the request is blocked before it is sent.
async fn allow_private_network(request: Request, next: Next) -> Response {
    let requested = request
        .headers()
        .get("access-control-request-private-network")
        .is_some_and(|value| value == "true");

    let mut response = next.run(request).await;
    if requested {
        response.headers_mut().insert(
            "access-control-allow-private-network",
            HeaderValue::from_static("true"),
        );
    }
    response
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
    use super::{HeaderValue, is_local_origin, redact};

    fn origin(value: &str) -> HeaderValue {
        HeaderValue::from_str(value).expect("a valid header value")
    }

    #[test]
    fn only_local_origins_may_drive_a_studio() {
        for allowed in [
            "http://localhost:5173",
            "http://127.0.0.1:4983",
            "http://[::1]:4983",
            "http://192.168.1.20:4983",
            "http://10.0.0.5:4983",
            "http://172.16.4.4:4983",
            "http://100.101.102.103:4983",
            "http://[fd7a:115c:a1e0::1]:4983",
            "http://mrbeast.tail5261d.ts.net:4983",
            "http://macbook.local:4983",
        ] {
            assert!(
                is_local_origin(&origin(allowed)),
                "{allowed} should be allowed"
            );
        }

        for refused in [
            "https://evil.example",
            "https://studio.besu.specta.dev.evil.example",
            "http://8.8.8.8:4983",
            "http://172.32.0.1:4983",
            "ftp://localhost",
            "null",
        ] {
            assert!(
                !is_local_origin(&origin(refused)),
                "{refused} should be refused"
            );
        }
    }

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
