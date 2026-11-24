//! The studio's API, exercised through the router with no server and no network.
//!
//! Each test seeds a real SQLite file in a temporary directory, builds the same router the
//! binary serves, and sends one request through `tower::ServiceExt::oneshot`.

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
    response::Response,
};
use besu::Fragment;
use besu_any::AnyDatabase;
use besu_studio::router;
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

/// A seeded studio and the temporary directory keeping its database file alive.
struct Fixture {
    app: axum::Router,
    _directory: TempDir,
}

/// Create a database with two related tables, a keyless table, a view and three users.
async fn fixture() -> Fixture {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("studio.db");
    let url = format!("sqlite://{}", path.display());
    let db = AnyDatabase::connect(&url)
        .await
        .expect("the seeded SQLite database");

    for statement in [
        "CREATE TABLE users (\
             id INTEGER PRIMARY KEY, \
             name TEXT NOT NULL, \
             active BOOLEAN NOT NULL\
         )",
        "CREATE TABLE posts (\
             id INTEGER PRIMARY KEY, \
             user_id INTEGER NOT NULL REFERENCES users(id), \
             title TEXT NOT NULL\
         )",
        "CREATE TABLE notes (body TEXT)",
        "INSERT INTO users (id, name, active) VALUES \
             (1, 'Ada', 1), (2, 'Grace', 0), (3, 'Alan', 1)",
        "INSERT INTO posts (id, user_id, title) VALUES (1, 1, 'On computable numbers')",
        "CREATE VIEW active_users AS SELECT id, name FROM users WHERE active = 1",
    ] {
        db.execute(Fragment::new(statement))
            .await
            .expect("the seed statement");
    }

    Fixture {
        app: router(db, url),
        _directory: directory,
    }
}

/// Send a request and hand back the response.
async fn send(app: &axum::Router, request: Request<Body>) -> Response {
    app.clone()
        .oneshot(request)
        .await
        .expect("the router to answer")
}

/// Send a `GET` and hand back the response.
async fn get(app: &axum::Router, uri: &str) -> Response {
    let request = Request::get(uri).body(Body::empty()).expect("a request");
    send(app, request).await
}

/// Send a `GET` and decode the body as JSON.
async fn get_json(app: &axum::Router, uri: &str) -> Value {
    let response = get(app, uri).await;
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    body_json(response).await
}

/// Send a request with a JSON body.
async fn request_json(app: &axum::Router, method: Method, uri: &str, body: Value) -> Response {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("a request");
    send(app, request).await
}

/// Send a `POST` with a JSON body.
async fn post_json(app: &axum::Router, uri: &str, body: Value) -> Response {
    request_json(app, Method::POST, uri, body).await
}

/// Read a response body as JSON.
async fn body_json(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the response body");
    serde_json::from_slice(&bytes).expect("a JSON response body")
}

/// Percent-encode a query parameter value, so a JSON filter survives the URI.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Read the rows of `users` through the API, for checking that nothing changed.
async fn users(app: &axum::Router) -> Vec<Value> {
    get_json(app, "/api/tables/users/rows?limit=500")
        .await
        .get("rows")
        .and_then(Value::as_array)
        .cloned()
        .expect("a rows array")
}

#[tokio::test]
async fn info_names_the_connection() {
    let fixture = fixture().await;
    let info = get_json(&fixture.app, "/api/info").await;

    assert_eq!(info["dialect"], "sqlite");
    assert_eq!(info["database"], "studio.db");
    assert_eq!(info["read_only"], false);
    assert!(info["version"].as_str().is_some_and(|v| !v.is_empty()));
}

#[tokio::test]
async fn schema_lists_the_tables() {
    let fixture = fixture().await;
    let schema = get_json(&fixture.app, "/api/schema").await;

    let tables = schema["tables"].as_array().expect("a tables array");
    let names: Vec<&str> = tables
        .iter()
        .filter_map(|table| table["name"].as_str())
        .collect();

    assert!(names.contains(&"users"), "{names:?}");
    assert!(names.contains(&"posts"), "{names:?}");

    let users = tables
        .iter()
        .find(|table| table["name"] == "users")
        .expect("the users table");
    let columns: Vec<&str> = users["columns"]
        .as_array()
        .expect("a columns array")
        .iter()
        .filter_map(|column| column["name"].as_str())
        .collect();
    assert_eq!(columns, ["id", "name", "active"]);
}

#[tokio::test]
async fn rows_are_typed_filtered_sorted_and_counted() {
    let fixture = fixture().await;

    let filter = encode(r#"[{"column":"active","op":"eq","value":"1"}]"#);
    let page = get_json(
        &fixture.app,
        &format!("/api/tables/users/rows?limit=50&offset=0&sort=name:desc&filter={filter}"),
    )
    .await;

    // `total` counts the filtered table, not the page.
    assert_eq!(page["total"], 2);
    assert_eq!(page["limit"], 50);
    assert_eq!(page["offset"], 0);
    assert_eq!(page["primary_key"], json!(["id"]));

    let columns = page["columns"].as_array().expect("a columns array");
    assert_eq!(columns[0]["name"], "id");
    assert_eq!(columns[0]["primary_key"], true);
    assert_eq!(columns[1]["ty"], "Text");
    // `active` is BOOLEAN, so the introspected type turns "1" back into a JSON boolean.
    assert_eq!(columns[2]["ty"], "Bool");

    assert_eq!(page["rows"], json!([[3, "Alan", true], [1, "Ada", true]]));

    // The primary key follows the requested sort, so two rows with the same `active` keep a
    // stable order between page requests.
    let ordered = get_json(&fixture.app, "/api/tables/users/rows?sort=active:asc").await;
    assert_eq!(
        ordered["rows"],
        json!([[2, "Grace", false], [1, "Ada", true], [3, "Alan", true]])
    );

    // A limit above the cap is clamped rather than refused.
    let capped = get_json(&fixture.app, "/api/tables/users/rows?limit=9999").await;
    assert_eq!(capped["limit"], 500);
}

#[tokio::test]
async fn a_view_is_read_through_a_probe() {
    let fixture = fixture().await;

    // SQLite lists a view under `views` only, with no columns, so the column names and types
    // come from `SELECT * FROM active_users LIMIT 1`.
    let page = get_json(&fixture.app, "/api/tables/active_users/rows").await;
    assert_eq!(page["table"], "active_users");
    assert_eq!(page["total"], 2);
    assert_eq!(page["primary_key"], json!([]));

    let columns = page["columns"].as_array().expect("a columns array");
    assert_eq!(columns[0]["name"], "id");
    assert_eq!(columns[0]["ty"], "Text");

    let rows = page["rows"].as_array().expect("a rows array");
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.is_array()));
    assert!(
        rows.iter().all(|row| row
            .as_array()
            .is_some_and(|row| row.iter().all(Value::is_string))),
        "a probed view has no introspected types, so every cell is a string: {rows:?}"
    );
}

#[tokio::test]
async fn a_json_string_round_trips_as_a_valid_json_document() {
    let fixture = fixture().await;
    let app = &fixture.app;
    let response = post_json(
        app,
        "/api/query",
        json!({"sql": "CREATE TABLE documents (id INTEGER PRIMARY KEY, payload JSON, label TEXT)"}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let uri = "/api/tables/documents/rows";
    let response = post_json(
        app,
        uri,
        json!({"values": {"id": 1, "payload": "a string", "label": "a string"}}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = get_json(app, uri).await;
    assert_eq!(page["rows"], json!([[1, "a string", "a string"]]));

    let response = request_json(
        app,
        Method::PATCH,
        uri,
        json!({"key": {"id": 1}, "values": {"payload": page["rows"][0][1]}}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    // An invalid document can still look like a string in the grid, so check what was stored.
    let response = post_json(
        app,
        "/api/query",
        json!({"sql": "SELECT json_valid(payload) AS valid, payload, label FROM documents"}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_json(response).await["rows"],
        json!([["1", "\"a string\"", "a string"]])
    );
}

#[tokio::test]
async fn insert_update_and_delete_round_trip() {
    let fixture = fixture().await;
    assert_eq!(users(&fixture.app).await.len(), 3);

    let inserted = post_json(
        &fixture.app,
        "/api/tables/users/rows",
        json!({"values": {"id": 4, "name": "Edsger", "active": true}}),
    )
    .await;
    assert_eq!(inserted.status(), StatusCode::OK);
    assert_eq!(body_json(inserted).await["inserted"], 1);
    assert_eq!(users(&fixture.app).await.len(), 4);

    let updated = request_json(
        &fixture.app,
        Method::PATCH,
        "/api/tables/users/rows",
        json!({"key": {"id": 4}, "values": {"name": "Edsger W."}}),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    assert_eq!(body_json(updated).await["updated"], 1);
    assert!(
        users(&fixture.app)
            .await
            .iter()
            .any(|row| row == &json!([4, "Edsger W.", true]))
    );

    let deleted = request_json(
        &fixture.app,
        Method::DELETE,
        "/api/tables/users/rows",
        json!({"key": {"id": 4}}),
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::OK);
    assert_eq!(body_json(deleted).await["deleted"], 1);
    assert_eq!(users(&fixture.app).await.len(), 3);
}

#[tokio::test]
async fn a_keyless_table_refuses_to_guess_a_row() {
    let fixture = fixture().await;

    let inserted = post_json(
        &fixture.app,
        "/api/tables/notes/rows",
        json!({"values": {"body": "hello"}}),
    )
    .await;
    assert_eq!(inserted.status(), StatusCode::OK);
    assert_eq!(body_json(inserted).await["inserted"], 1);

    let updated = request_json(
        &fixture.app,
        Method::PATCH,
        "/api/tables/notes/rows",
        json!({"key": {"body": "hello"}, "values": {"body": "goodbye"}}),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::BAD_REQUEST);
    let error = body_json(updated).await;
    assert!(
        error["error"]
            .as_str()
            .is_some_and(|message| message.contains("no primary key")),
        "{error}"
    );

    let deleted = request_json(
        &fixture.app,
        Method::DELETE,
        "/api/tables/notes/rows",
        json!({"key": {"body": "hello"}}),
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::BAD_REQUEST);

    // A primary key that is present but incomplete is refused for the same reason.
    let incomplete = request_json(
        &fixture.app,
        Method::PATCH,
        "/api/tables/users/rows",
        json!({"key": {"name": "Ada"}, "values": {"name": "Ada L."}}),
    )
    .await;
    assert_eq!(incomplete.status(), StatusCode::BAD_REQUEST);
    let error = body_json(incomplete).await;
    assert!(
        error["error"]
            .as_str()
            .is_some_and(|message| message.contains("missing primary-key column `id`")),
        "{error}"
    );
}

#[tokio::test]
async fn a_select_returns_typed_columns_and_string_cells() {
    let fixture = fixture().await;

    let response = post_json(
        &fixture.app,
        "/api/query",
        json!({"sql": "SELECT id, name, active FROM users ORDER BY id"}),
    )
    .await;

    let status = response.status();
    let result = body_json(response).await;
    assert_eq!(status, StatusCode::OK, "{result}");

    assert_eq!(result["columns"], json!(["id", "name", "active"]));
    assert_eq!(
        result["rows"],
        json!([["1", "Ada", "1"], ["2", "Grace", "0"], ["3", "Alan", "1"],])
    );
    assert_eq!(result["rows_affected"], 0);
    assert!(result["duration_ms"].is_number(), "{result}");
}

#[tokio::test]
async fn binary_rows_are_hex_encoded_and_other_pages_still_work() {
    let fixture = fixture().await;
    let created = post_json(
        &fixture.app,
        "/api/query",
        json!({"sql": "CREATE TABLE blobs (id INTEGER PRIMARY KEY, data BLOB, label TEXT)"}),
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK);
    let inserted = post_json(
        &fixture.app,
        "/api/query",
        json!({"sql": "INSERT INTO blobs VALUES (1, X'FF', 'first'), (2, X'4869', 'second')"}),
    )
    .await;
    assert_eq!(inserted.status(), StatusCode::OK);

    let first = get_json(&fixture.app, "/api/tables/blobs/rows?limit=1").await;
    assert_eq!(first["rows"], json!([[1, "hex:FF", "first"]]));
    let second = get_json(&fixture.app, "/api/tables/blobs/rows?limit=1&offset=1").await;
    assert_eq!(second["rows"], json!([[2, "hex:4869", "second"]]));

    let updated = request_json(
        &fixture.app,
        Method::PATCH,
        "/api/tables/blobs/rows",
        json!({"key": {"id": 1}, "values": {"data": "hex:00FF"}}),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let page = get_json(&fixture.app, "/api/tables/blobs/rows?limit=1").await;
    assert_eq!(page["rows"], json!([[1, "hex:00FF", "first"]]));
}

#[tokio::test]
async fn the_query_console_returns_pragma_and_returning_results() {
    let fixture = fixture().await;

    let pragma = post_json(
        &fixture.app,
        "/api/query",
        json!({"sql": "PRAGMA table_info(users)"}),
    )
    .await;
    assert_eq!(pragma.status(), StatusCode::OK);
    let result = body_json(pragma).await;
    assert_eq!(
        result["columns"],
        json!(["cid", "name", "type", "notnull", "dflt_value", "pk"])
    );
    assert_eq!(result["rows"].as_array().expect("rows").len(), 3);

    let returned = post_json(
        &fixture.app,
        "/api/query",
        json!({"sql": "INSERT INTO users (id, name, active) VALUES (4, 'Ada', 1) RETURNING id, name"}),
    )
    .await;
    assert_eq!(returned.status(), StatusCode::OK);
    let result = body_json(returned).await;
    assert_eq!(result["columns"], json!(["id", "name"]));
    assert_eq!(result["rows"], json!([["4", "Ada"]]));
    assert_eq!(result["rows_affected"], 1);
    assert_eq!(users(&fixture.app).await.len(), 4);

    let updated = post_json(
        &fixture.app,
        "/api/query",
        json!({"sql": "UPDATE users SET active = 0 WHERE id = 4"}),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let result = body_json(updated).await;
    assert_eq!(result["columns"], json!([]));
    assert_eq!(result["rows"], json!([]));
    assert_eq!(result["rows_affected"], 1);
}

#[tokio::test]
async fn a_with_select_returns_its_rows() {
    let fixture = fixture().await;
    let response = post_json(
        &fixture.app,
        "/api/query",
        json!({"sql": "WITH ids AS (SELECT id FROM users) SELECT * FROM ids ORDER BY id"}),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let result = body_json(response).await;
    assert_eq!(result["columns"], json!(["id"]));
    assert_eq!(result["rows"], json!([["1"], ["2"], ["3"]]));
}

#[tokio::test]
async fn a_later_console_query_waits_for_an_inflight_statement() {
    use std::time::Duration;

    let fixture = fixture().await;
    let app = fixture.app.clone();
    let first = tokio::spawn(async move {
        post_json(
            &app,
            "/api/query",
            json!({
                "sql": "WITH RECURSIVE n(x) AS (
                    VALUES(1) UNION ALL SELECT x + 1 FROM n WHERE x < 1000000
                ) SELECT sum(x) AS total FROM n"
            }),
        )
        .await
    });

    // Give the first request time to acquire the console lock and start its
    // deliberately expensive read. The later request must queue behind it.
    tokio::time::sleep(Duration::from_millis(10)).await;
    let started = std::time::Instant::now();
    let second = post_json(
        &fixture.app,
        "/api/query",
        json!({"sql": "SELECT 1 AS one"}),
    )
    .await;
    let waited = started.elapsed();

    assert!(
        waited >= Duration::from_millis(50),
        "only waited {waited:?}"
    );
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(body_json(second).await["rows"], json!([["1"]]));

    let first = first.await.expect("the first request finishes");
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(body_json(first).await["rows"], json!([["500000500000"]]));
}

#[tokio::test]
async fn unsafe_64_bit_keys_keep_their_identity_through_update_and_delete() {
    let fixture = fixture().await;
    let created = post_json(
        &fixture.app,
        "/api/query",
        json!({"sql": "CREATE TABLE precision_rows (id INTEGER PRIMARY KEY, label TEXT NOT NULL)"}),
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK);
    let inserted = post_json(
        &fixture.app,
        "/api/query",
        json!({"sql": "INSERT INTO precision_rows VALUES (9007199254740992, 'first'), (9007199254740993, 'second')"}),
    )
    .await;
    assert_eq!(inserted.status(), StatusCode::OK);

    let page = get_json(&fixture.app, "/api/tables/precision_rows/rows").await;
    assert_eq!(
        page["rows"],
        json!([
            ["9007199254740992", "first"],
            ["9007199254740993", "second"]
        ])
    );

    let updated = request_json(
        &fixture.app,
        Method::PATCH,
        "/api/tables/precision_rows/rows",
        json!({"key": {"id": "9007199254740993"}, "values": {"label": "changed"}}),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    assert_eq!(body_json(updated).await["updated"], 1);
    let page = get_json(&fixture.app, "/api/tables/precision_rows/rows").await;
    assert_eq!(page["rows"][0][1], "first");
    assert_eq!(page["rows"][1][1], "changed");

    let deleted = request_json(
        &fixture.app,
        Method::DELETE,
        "/api/tables/precision_rows/rows",
        json!({"key": {"id": "9007199254740993"}}),
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::OK);
    assert_eq!(body_json(deleted).await["deleted"], 1);
    let page = get_json(&fixture.app, "/api/tables/precision_rows/rows").await;
    assert_eq!(page["rows"], json!([["9007199254740992", "first"]]));
}

#[tokio::test]
async fn a_statement_without_rows_reports_the_changes() {
    let fixture = fixture().await;

    let response = post_json(
        &fixture.app,
        "/api/query",
        json!({"sql": "INSERT INTO users (id, name, active) VALUES (4, 'Edsger', 1)"}),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let result = body_json(response).await;
    assert_eq!(result["columns"], json!([]));
    assert_eq!(result["rows"], json!([]));
    assert_eq!(result["rows_affected"], 1);
    assert_eq!(users(&fixture.app).await.len(), 4);

    // The probe succeeded but found no row, so there are no names to show either.
    let empty = post_json(
        &fixture.app,
        "/api/query",
        json!({"sql": "SELECT * FROM users WHERE 0"}),
    )
    .await;
    assert_eq!(empty.status(), StatusCode::OK);
    let result = body_json(empty).await;
    assert_eq!(result["columns"], json!([]));
    assert_eq!(result["rows"], json!([]));
    assert_eq!(result["rows_affected"], 0);
}

#[tokio::test]
async fn errors_are_json_bodies() {
    let fixture = fixture().await;

    let missing = get(&fixture.app, "/api/tables/missing/rows").await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let error = body_json(missing).await;
    assert!(
        error["error"]
            .as_str()
            .is_some_and(|message| message.contains("no table or view")),
        "{error}"
    );

    let bad_filter = get(&fixture.app, "/api/tables/users/rows?filter=not-json").await;
    assert_eq!(bad_filter.status(), StatusCode::BAD_REQUEST);
    let error = body_json(bad_filter).await;
    assert!(
        error["error"]
            .as_str()
            .is_some_and(|message| message.contains("filter")),
        "{error}"
    );

    let multiple = post_json(
        &fixture.app,
        "/api/query",
        json!({"sql": "DELETE FROM users; DELETE FROM posts"}),
    )
    .await;
    assert_eq!(multiple.status(), StatusCode::BAD_REQUEST);
    let error = body_json(multiple).await;
    assert!(
        error["error"]
            .as_str()
            .is_some_and(|message| message.contains("exactly one statement")),
        "{error}"
    );

    // Refusing the request must mean refusing every part of it.
    assert_eq!(users(&fixture.app).await.len(), 3);
}

#[tokio::test]
async fn an_unknown_table_touches_nothing() {
    let fixture = fixture().await;
    assert_eq!(users(&fixture.app).await.len(), 3);

    // The table name comes from the URL, so a statement smuggled into it must not run.
    let injected = get(
        &fixture.app,
        "/api/tables/users%3B%20DROP%20TABLE%20users/rows",
    )
    .await;
    assert_eq!(injected.status(), StatusCode::NOT_FOUND);

    assert_eq!(users(&fixture.app).await.len(), 3);
}

#[tokio::test]
async fn the_page_is_html() {
    let fixture = fixture().await;
    let response = get(&fixture.app, "/").await;

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .expect("a content type");
    assert!(content_type.starts_with("text/html"), "{content_type}");

    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the page body");
    let page = String::from_utf8(body.to_vec()).expect("a UTF-8 page");
    assert!(page.contains("besu studio"));
    assert!(page.contains("/assets/index-"));
    assert!(page.contains(r#"<div id="root">"#));
}
