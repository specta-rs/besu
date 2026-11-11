//! The studio's API, exercised through the router with no server and no network.
//!
//! Each test seeds a real SQLite file in a temporary directory, builds the same router the
//! binary serves, and sends one request through `tower::ServiceExt::oneshot`.

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
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

/// Create a database with two related tables and three rows in `users`.
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
        "INSERT INTO users (id, name, active) VALUES \
             (1, 'Ada', 1), (2, 'Grace', 0), (3, 'Alan', 1)",
        "INSERT INTO posts (id, user_id, title) VALUES (1, 1, 'On computable numbers')",
    ] {
        db.execute(Fragment::new(statement))
            .await
            .expect("the seed statement");
    }

    Fixture {
        app: router(db),
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

/// Send a `POST` with a JSON body.
async fn post_json(app: &axum::Router, uri: &str, body: Value) -> Response {
    let request = Request::post(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("a request");
    send(app, request).await
}

/// Read a response body as JSON.
async fn body_json(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the response body");
    serde_json::from_slice(&bytes).expect("a JSON response body")
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
async fn rows_respect_limit_and_offset() {
    let fixture = fixture().await;

    let first = get_json(&fixture.app, "/api/tables/users/rows?limit=2").await;
    assert_eq!(first["limit"], 2);
    assert_eq!(first["offset"], 0);
    assert_eq!(first["columns"], json!(["id", "name", "active"]));
    let rows = first["rows"].as_array().expect("a rows array");
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .all(|row| row.as_array().is_some_and(|row| row.len() == 3))
    );

    // `active` is BOOLEAN, so the introspected type turns "1" back into a JSON boolean.
    let ada = rows
        .iter()
        .find(|row| row[1] == "Ada")
        .expect("Ada on the first page");
    assert_eq!(ada[2], json!(true));

    let second = get_json(&fixture.app, "/api/tables/users/rows?limit=2&offset=2").await;
    assert_eq!(second["rows"].as_array().expect("a rows array").len(), 1);

    // A limit above the cap is clamped rather than refused.
    let capped = get_json(&fixture.app, "/api/tables/users/rows?limit=9999").await;
    assert_eq!(capped["limit"], 500);
}

#[tokio::test]
async fn an_unknown_table_is_a_404_and_touches_nothing() {
    let fixture = fixture().await;
    assert_eq!(users(&fixture.app).await.len(), 3);

    let missing = get(&fixture.app, "/api/tables/missing/rows").await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    // The table name comes from the URL, so a statement smuggled into it must not run.
    let injected = get(
        &fixture.app,
        "/api/tables/users%3B%20DROP%20TABLE%20users/rows",
    )
    .await;
    assert_eq!(injected.status(), StatusCode::NOT_FOUND);

    assert_eq!(users(&fixture.app).await.len(), 3);

    let schema = get_json(&fixture.app, "/api/schema").await;
    let names: Vec<&str> = schema["tables"]
        .as_array()
        .expect("a tables array")
        .iter()
        .filter_map(|table| table["name"].as_str())
        .collect();
    assert!(
        names.contains(&"users"),
        "the injected name dropped the table"
    );
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
    assert!(page.contains("/api/schema"));
}

#[tokio::test]
async fn query_refuses_more_than_one_statement() {
    let fixture = fixture().await;

    let response = post_json(
        &fixture.app,
        "/api/query",
        json!({ "sql": "DELETE FROM users; DELETE FROM posts" }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error = body_json(response).await;
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
async fn query_runs_one_statement_and_reports_the_changes() {
    let fixture = fixture().await;

    let response = post_json(
        &fixture.app,
        "/api/query",
        json!({ "sql": "INSERT INTO users (id, name, active) VALUES (4, 'Edsger', 1);" }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["rows_affected"], 1);
    assert!(
        body["warning"]
            .as_str()
            .is_some_and(|warning| warning.contains("arbitrary SQL")),
        "{body}"
    );

    assert_eq!(users(&fixture.app).await.len(), 4);
}
