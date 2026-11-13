//! An enum declared from Rust on SQLite: there is no enum type, so the label is stored in a
//! `TEXT` column and round-trips through the model.
#![cfg(feature = "sqlx")]
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::prelude::*;
use besu::schema::Change;
use besu_sqlite::Sqlx;

#[derive(Clone, Copy, Debug, PartialEq, Eq, besu::SqlEnum)]
#[besu(rename_all = "snake_case")]
pub enum Mood {
    Happy,
    #[besu(rename = "meh")]
    Neutral,
    Sad,
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "reviews")]
pub struct Review {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    #[besu(sql_type = "Mood")]
    pub mood: Mood,
}

#[tokio::test]
async fn an_enum_is_text_and_round_trips() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());

    let first = db.push([reviews.def()]).await.unwrap();
    assert!(
        first
            .statements
            .iter()
            .any(|statement| statement.contains(r#""mood" TEXT NOT NULL"#)),
        "SQLite has no enum, so the label must be text: {:?}",
        first.statements
    );
    assert!(
        !first
            .changes
            .iter()
            .any(|change| matches!(change, Change::CreateEnum(_))),
        "SQLite has no named enum: {:?}",
        first.changes
    );

    db.insert(reviews)
        .value(reviews.insert(Mood::Neutral))
        .await
        .unwrap();

    let stored: Vec<Review> = db
        .select(reviews)
        .filter(reviews.mood.eq(Mood::Neutral))
        .await
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].mood, Mood::Neutral);

    // A label with no matching variant is a decode error that names the column, rather than
    // a silently invented variant.
    db.execute(sql!("INSERT INTO reviews (mood) VALUES ('angry')"))
        .await
        .unwrap();
    let error = db
        .select(reviews)
        .await
        .expect_err("an unknown label must not decode");
    let message = error.to_string();
    assert!(message.contains("angry"), "{message}");
    assert!(message.contains("column 1"), "{message}");

    // The text column converges, so the declaration does not report drift every push.
    let second = db.push([reviews.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);
}
