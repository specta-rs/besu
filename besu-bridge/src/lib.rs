//! Run a Besu query in a browser by sending it to a server.
//!
//! [`besu-core`](https://docs.rs/besu-core) and [`besu`](https://docs.rs/besu) already compile
//! for `wasm32-unknown-unknown`: a browser can build a typed query and render the exact SQL a
//! server will run. This crate makes the last step possible too. [`Bridge`] is a
//! [`Driver`](besu_core::Driver) that posts the rendered SQL and its bound parameters to an
//! HTTP endpoint, and decodes the rows it gets back into the same Rust types a server-side
//! driver would.
//!
//! The bridge is a transport, not a database engine. The server decides what actually runs the
//! statement, so the bridge is generic over the dialect whose spelling it emits:
//!
//! - `Bridge<FetchTransport, Postgres>` writes `$1` and quotes with `"`.
//! - `Bridge<FetchTransport, MySql>` writes `?` and quotes with `` ` ``.
//! - `Bridge<FetchTransport, SQLite>` writes `?` and SQLite's type names.
//!
//! The dialect's capabilities are forwarded too, so `RETURNING` on a MySQL bridge is rejected
//! while the query is being built rather than by the server.
//!
//! # Client
//!
//! ```ignore
//! use besu::prelude::*;
//! use besu_bridge::{Bridge, FetchTransport};
//! use besu_postgres::Postgres;
//!
//! let db = Database::new(Bridge::<_, Postgres>::new(
//!     FetchTransport::new("/api/query"),
//! ));
//!
//! let adults: Vec<User> = db.select(users).filter(users.age.gt(18)).await?;
//! ```
//!
//! # Server
//!
//! The endpoint receives a [`Request`], rebuilds a `Sql`, and runs it against whatever driver
//! it has. [`handle_request`] does the wiring through [`BridgeSource`], which is where a
//! server says how to read its driver's rows:
//!
//! ```ignore
//! use besu_bridge::{BridgeSource, Request, Response, handle_request};
//!
//! async fn endpoint(sqlx: &besu_postgres::Sqlx, body: String) -> Response {
//!     match serde_json::from_str::<Request>(&body) {
//!         Ok(request) => handle_request(sqlx, &PostgresRows, request).await,
//!         Err(err) => Response::error(format!("not a bridge request: {err}")),
//!     }
//! }
//! ```
//!
//! # The wire format
//!
//! A request is `{"sql": "...", "params": [...], "kind": "query" | "execute"}`. A response is
//! one of `{"columns": [...], "rows": [[...], ...]}`, `{"affected": n}` or
//! `{"error": {"message": "...", "code": "..."}}`. [`Request`], [`Response`] and
//! [`BridgeValue`] are ordinary `serde` types, so any language can implement the endpoint.
//!
//! Every value is a tagged object, `{"t": "<tag>", "v": <payload>}`:
//!
//! | Tag | Payload | Rust |
//! | --- | --- | --- |
//! | `null` | none | `()` / `Option<T>` |
//! | `bool` | JSON boolean | `bool` |
//! | `int` | JSON number | `i8` … `i64` |
//! | `uint` | JSON number | `u8` … `u64` |
//! | `uint128` | decimal **string** | `u128` |
//! | `int128` | decimal **string** | `i128` |
//! | `float` | JSON number, or `"nan"`, `"inf"`, `"-inf"` | `f32`, `f64` |
//! | `text` | JSON string | `String` |
//! | `enum_label` | JSON string | `String` (a SQL enum) |
//! | `bytes` | base64 string | `Vec<u8>` |
//! | `uuid` | hyphenated string | `uuid::Uuid` |
//! | `date` | `YYYY-MM-DD` | `chrono::NaiveDate` |
//! | `time` | `HH:MM:SS[.fraction]` | `chrono::NaiveTime` |
//! | `timestamp` | `YYYY-MM-DDTHH:MM:SS[.fraction]` | `chrono::NaiveDateTime` |
//! | `timestamp_tz` | RFC 3339 | `chrono::DateTime<Utc>` |
//! | `decimal` | exact decimal string | `String` |
//! | `json` | any JSON value | `serde_json::Value` |
//! | `array` | `{"element": <type>, "values": [...]}` | `Vec<T>` |
//! | `interval` | canonical text | `SqlInterval` |
//! | `inet` / `cidr` | canonical text | `Inet`, `Cidr` |
//! | `bit` | `0`/`1` string | `BitString` |
//! | `range` | `{"element": <type>, "bounds": ...}` | `Range<T>` |
//! | `geometry` | base64 EWKB | `Geometry` |
//! | `vector` | `[1,2,3]` text | `Vector` |
//!
//! The tags are load-bearing. `int` and `uint` stay JSON numbers because `serde` reads and
//! writes 64-bit integers exactly, though a peer written in JavaScript must parse them with
//! `BigInt` for magnitudes above `2^53`. The 128-bit variants are decimal strings because
//! `JSON.parse` has no representation for them at all. A decimal is text because its whole
//! point is that it is not a float, and a value is never quietly converted from one tag to
//! another on the way in or out.
//!
//! # Testing
//!
//! [`MemoryTransport`] answers from a closure and records every request, so a test can run the
//! whole path — build SQL, encode parameters, decode rows — without a server or a browser.
//! The crate has the same select-and-insert test in `tests/bridge.rs` on a native target and
//! `tests/browser.rs` under `wasm-bindgen-test`.
//!
//! # Limitations
//!
//! - A bridge cannot stream. The server sends one response, so [`Driver::query_stream`]
//!   collects through [`Driver::query`].
//! - There is no `TransactionDriver`: an HTTP request is its own transaction, and a
//!   transaction held open across requests would be a server-side session the bridge does not
//!   model.
//! - A result set with no rows has no column names, because [`Driver::column_names`] reads
//!   them off a row. A server that knows its columns can fill [`Response::columns`] itself
//!   instead of calling [`handle_request`].
//! - [`handle_request`] handles one statement per request. Batching is the client's job.
//! - The endpoint runs SQL the client sends, exactly as rendered. Authenticate and authorise
//!   it like any other SQL execution endpoint; the bridge adds no policy layer of its own.
//!
//! [`Driver::query_stream`]: besu_core::Driver::query_stream
//! [`Driver::query`]: besu_core::Driver::query
//! [`Driver::column_names`]: besu_core::Driver::column_names
//!
//! <section class="warning">
//!
//! Besu is in early development and is **not ready for public use!**
//!
//! </section>
#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]

mod base64;
mod decode;
mod dialect;
mod driver;
mod error;
#[cfg(target_arch = "wasm32")]
mod fetch;
mod server;
mod transport;
mod wire;

pub use dialect::BridgeDialect;
pub use driver::{Bridge, BridgeRow};
pub use error::BridgeError;
#[cfg(target_arch = "wasm32")]
#[cfg_attr(docsrs, doc(cfg(target_arch = "wasm32")))]
pub use fetch::FetchTransport;
pub use server::{BridgeSource, handle_request};
pub use transport::{MemoryTransport, Transport};
pub use wire::{
    BridgeArray, BridgeBound, BridgeRange, BridgeValue, Request, RequestKind, Response, ServerError,
};

/// A [`besu::Database`] backed by a bridge.
///
/// ```ignore
/// use besu_bridge::{Bridge, Database};
/// use besu_postgres::Postgres;
///
/// let db: Database<_, Postgres> = Database::new(Bridge::new(transport));
/// ```
pub type Database<T, D> = besu::Database<Bridge<T, D>>;
