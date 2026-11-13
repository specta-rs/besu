//! A select and an insert through the bridge, run inside a real browser.
//!
//! The transport answers from a closure, so no server is needed: what runs in the browser is
//! the part that only a browser can run — the `wasm32` build of the query builder, the bridge
//! driver, and its decoding. `FetchTransport` is covered by the native test's type-checking
//! and by any application that points it at a server; a browser test cannot reach a real
//! endpoint without one.
//!
//! Run it with:
//!
//! ```text
//! rustup target add wasm32-unknown-unknown
//! cargo install wasm-bindgen-cli --version <the lockfile's wasm-bindgen>
//! cargo test --target wasm32-unknown-unknown -p besu-bridge --test browser
//! ```

#![cfg(target_arch = "wasm32")]
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::prelude::*;
use besu_bridge::{Bridge, BridgeValue, MemoryTransport, RequestKind, Response};
use besu_postgres::Postgres;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "users")]
pub struct User {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub email: String,
    pub name: String,
    pub age: i32,
    pub bio: Option<String>,
}

fn columns() -> Vec<String> {
    ["id", "email", "name", "age", "bio"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

fn oscar() -> User {
    User {
        id: 1,
        email: "oscar@example.com".to_owned(),
        name: "Oscar".to_owned(),
        age: 21,
        bio: None,
    }
}

fn oscar_row() -> Vec<BridgeValue> {
    vec![
        BridgeValue::Int(1),
        BridgeValue::Text("oscar@example.com".to_owned()),
        BridgeValue::Text("Oscar".to_owned()),
        BridgeValue::Int(21),
        BridgeValue::Null,
    ]
}

#[wasm_bindgen_test]
async fn select_and_insert_through_the_bridge() {
    // A select: the browser renders PostgreSQL SQL, posts it, and decodes a typed model.
    let transport = MemoryTransport::new(|request| {
        assert_eq!(request.kind, RequestKind::Query);
        assert!(
            request.sql.contains("$1"),
            "expected a PostgreSQL placeholder in `{}`",
            request.sql
        );
        assert_eq!(request.params, vec![BridgeValue::Int(1)]);
        Ok(Response::rows(Some(columns()), vec![oscar_row()]))
    });
    let db = besu::Database::new(Bridge::<_, Postgres>::new(transport.clone()));

    let rows: Vec<User> = db.select(users).filter(users.id.eq(1)).await.unwrap();
    assert_eq!(rows, vec![oscar()]);
    assert!(transport.last_request().unwrap().sql.starts_with("SELECT"));

    // An insert that returns no rows: the response is an affected-row count.
    let transport = MemoryTransport::new(|request| {
        assert_eq!(request.kind, RequestKind::Execute);
        assert_eq!(
            request.params,
            vec![
                BridgeValue::Text("oscar@example.com".to_owned()),
                BridgeValue::Text("Oscar".to_owned()),
                BridgeValue::Int(21),
            ]
        );
        Ok(Response::affected(1))
    });
    let db = besu::Database::new(Bridge::<_, Postgres>::new(transport.clone()));

    let affected: u64 = db
        .insert(users)
        .value(users.insert("oscar@example.com", "Oscar", 21))
        .await
        .unwrap();
    assert_eq!(affected, 1);
    assert!(
        transport
            .last_request()
            .unwrap()
            .sql
            .starts_with("INSERT INTO")
    );
}

#[wasm_bindgen_test]
async fn insert_returning_decodes_the_written_row() {
    let transport = MemoryTransport::new(|request| {
        assert_eq!(request.kind, RequestKind::Query);
        assert!(request.sql.contains("RETURNING"));
        Ok(Response::rows(Some(columns()), vec![oscar_row()]))
    });
    let db = besu::Database::new(Bridge::<_, Postgres>::new(transport));

    let created: Vec<User> = db
        .insert(users)
        .value(users.insert("oscar@example.com", "Oscar", 21))
        .returning(users)
        .await
        .unwrap();
    assert_eq!(created, vec![oscar()]);
}

#[wasm_bindgen_test]
async fn a_server_error_is_a_rust_error() {
    let transport = MemoryTransport::new(|_| Ok(Response::error("that row already exists")));
    let db = besu::Database::new(Bridge::<_, Postgres>::new(transport));

    let error = db.select(users).await.unwrap_err();
    assert!(error.to_string().contains("that row already exists"));
}
