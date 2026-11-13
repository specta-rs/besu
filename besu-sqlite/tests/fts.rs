//! Full-text search against an FTS5 virtual table.
//!
//! SQLite's `MATCH` is a property of the table rather than a function, so the table has to be
//! an FTS5 one. The schema layer cannot declare a virtual table, so this creates it with raw
//! SQL and queries it through an ordinary table handle.
#![cfg(feature = "sqlx")]
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::prelude::*;
use besu_sqlite::{SQLite, Sqlx};

/// FTS5 owns the storage and the rowid, so the model describes only the indexed columns.
#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "documents")]
pub struct Document {
    pub body: String,
}

async fn setup() -> besu::Database<Sqlx> {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.execute(sql!("CREATE VIRTUAL TABLE documents USING fts5(body)"))
        .await
        .unwrap();
    db.insert(documents)
        .values([
            documents.insert("the quick brown fox"),
            documents.insert("a slow green turtle"),
        ])
        .await
        .unwrap();
    db
}

#[tokio::test]
async fn matches_an_fts5_table() {
    let db = setup().await;

    let found: Vec<Document> = db
        .select(documents)
        .filter(documents.body.matches("quick"))
        .await
        .unwrap();
    assert_eq!(
        found,
        [Document {
            body: "the quick brown fox".into()
        }]
    );

    // An unrelated word matches nothing; `MATCH` is a word search, not a substring test.
    let unrelated: Vec<Document> = db
        .select(documents)
        .filter(matches(documents.body, "zebra"))
        .await
        .unwrap();
    assert!(unrelated.is_empty());

    // FTS5 parses the bound text, so its own query syntax works without the caller writing
    // any SQL: a quoted phrase is a phrase search.
    let phrase: Vec<Document> = db
        .select(documents)
        .filter(documents.body.matches("\"brown fox\""))
        .await
        .unwrap();
    assert_eq!(phrase.len(), 1);
}

#[tokio::test]
async fn renders_a_bound_match() {
    let db = besu::dry::<SQLite>();
    let sql = db
        .select(documents)
        .filter(documents.body.matches("quick"))
        .to_sql()
        .unwrap();

    assert_eq!(
        sql.sql(),
        r#"SELECT "documents"."body" FROM "documents" WHERE "documents"."body" MATCH ?"#
    );
    assert_eq!(sql.args(), [Value::Text("quick".into())]);
}
