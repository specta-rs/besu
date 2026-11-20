//! Row-shape checks against a real SQLite result.
#![cfg(feature = "native")]
#![allow(clippy::unwrap_used)]

use besu::{Error, sql};
use besu_sqlite::{Native, NativeError};

#[tokio::test]
async fn an_optional_decode_distinguishes_missing_columns_from_nulls() {
    let db = Native::in_memory().unwrap().database();

    let missing = db
        .execute(sql!("SELECT 1"))
        .first::<(i64, Option<i64>)>()
        .await;
    assert!(matches!(
        missing,
        Err(NativeError::Core(Error::Decode { column: 1, .. }))
    ));

    let partial = db
        .execute(sql!("SELECT NULL"))
        .first::<Option<(i64, i64)>>()
        .await;
    assert!(matches!(
        partial,
        Err(NativeError::Core(Error::Decode { column: 1, .. }))
    ));

    let nulls = db
        .execute(sql!("SELECT NULL, NULL"))
        .first::<Option<(i64, i64)>>()
        .await
        .unwrap();
    assert_eq!(nulls, Some(None));
}
