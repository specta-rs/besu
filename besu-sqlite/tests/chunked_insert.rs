//! Tests for chunked inserts against SQLite.
#![cfg(feature = "native")]
#![allow(clippy::unwrap_used, missing_docs)]

use besu::prelude::*;
use besu_sqlite::Native;

/// A row with a database-assigned key to track insertion order.
#[derive(Table)]
pub struct Item {
    #[besu(primary_key, auto_increment)]
    id: i64,
    name: String,
}

#[tokio::test]
async fn chunked_inserts_include_previously_added_rows() {
    let db = Native::in_memory().unwrap().database();
    db.push([item.def()]).await.unwrap();

    let outputs = db
        .insert(item)
        .value(item.insert("first"))
        .values([item.insert("second")])
        .values_chunked([item.insert("third")], 2)
        .await
        .unwrap();

    let names = db
        .select(item)
        .pick(item.name)
        .order_by(item.id.asc())
        .await
        .unwrap();
    assert_eq!(names, ["first", "second", "third"]);
    assert_eq!(
        outputs
            .iter()
            .map(|out| out.rows_affected())
            .collect::<Vec<_>>(),
        [2, 1]
    );
}
