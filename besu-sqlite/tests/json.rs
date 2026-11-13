//! JSON path access renders correctly per dialect and reads real documents.
#![cfg(feature = "sqlx")]
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::prelude::*;
use besu_sqlite::{SQLite, Sqlx};

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "documents")]
pub struct Document {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub body: serde_json::Value,
}

#[tokio::test]
async fn reads_a_nested_key_as_json_and_as_text() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.push([documents.def()]).await.unwrap();
    db.insert(documents)
        .value(documents.insert(serde_json::json!({
            "user": { "name": "Oscar", "age": 21 },
            "tags": ["a", "b"]
        })))
        .await
        .unwrap();

    let text: Vec<String> = db
        .select(documents)
        .pick(json_text(documents.body, "user.name"))
        .await
        .unwrap();
    assert_eq!(text, ["Oscar"]);

    // The text form unquotes, so it compares against a parameter like any other string.
    let matching: Vec<Document> = db
        .select(documents)
        .filter(json_text(documents.body, "user.name").eq("Oscar"))
        .await
        .unwrap();
    assert_eq!(matching.len(), 1);

    // The JSON form keeps the document's type, so it decodes as a JSON value rather than
    // pretending a jsonb can be compared to an integer.
    let ages: Vec<serde_json::Value> = db
        .select(documents)
        .pick(json_get(documents.body, "user.age"))
        .await
        .unwrap();
    assert_eq!(ages, [serde_json::json!(21)]);

    let tags: Vec<serde_json::Value> = db
        .select(documents)
        .pick(json_get(documents.body, "tags"))
        .await
        .unwrap();
    assert_eq!(tags, [serde_json::json!(["a", "b"])]);
}

#[tokio::test]
async fn renders_per_dialect() {
    let sqlite = besu::dry::<SQLite>();
    let sql = sqlite
        .select(documents)
        .pick(json_text(documents.body, "user.name"))
        .to_sql()
        .unwrap();
    assert!(
        sql.sql()
            .contains("\"documents\".\"body\" ->> '$.user.name'"),
        "got {}",
        sql.sql()
    );
}
