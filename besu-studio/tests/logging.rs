//! INFO-level request logging regressions for the real Studio router.

use std::sync::{Arc, Mutex};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use besu::Fragment;
use besu_any::AnyDatabase;
use besu_studio::router;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("the log lock")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Logs {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(self.0.clone())
    }
}

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

#[tokio::test]
async fn info_request_logs_do_not_contain_filter_values() {
    use tracing::instrument::WithSubscriber;

    let _directory: TempDir = tempfile::tempdir().expect("a temporary directory");
    let path = _directory.path().join("studio.db");
    let url = format!("sqlite://{}", path.display());
    let db = AnyDatabase::connect(&url).await.expect("open SQLite");
    for statement in [
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, active BOOLEAN NOT NULL)",
        "INSERT INTO users VALUES (1, 'AUDIT_FILTER_SECRET_7f3a', 1)",
    ] {
        db.execute(Fragment::new(statement))
            .await
            .expect("seed SQLite");
    }
    let app = router(db, url);

    let filter = encode(r#"[{"column":"name","op":"eq","value":"AUDIT_FILTER_SECRET_7f3a"}]"#);
    let logs = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
        .with_writer(Logs(logs.clone()))
        .without_time()
        .finish();

    let response = async {
        app.oneshot(
            Request::get(format!(
                "/api/tables/users/rows?limit=50&offset=0&filter={filter}"
            ))
            .body(Body::empty())
            .expect("a request"),
        )
        .await
        .expect("the router answers")
    }
    .with_subscriber(subscriber)
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the response body");
    let page: Value = serde_json::from_slice(&bytes).expect("a JSON response");
    assert_eq!(
        page["rows"],
        serde_json::json!([[1, "AUDIT_FILTER_SECRET_7f3a", true]])
    );

    let output = String::from_utf8(logs.lock().expect("the log lock").clone()).expect("UTF-8 logs");
    assert!(output.contains("filter=%5Bredacted%5D"), "{output}");
    assert!(!output.contains("AUDIT_FILTER_SECRET_7f3a"), "{output}");
}
