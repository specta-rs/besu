//! Julian-day timestamps decoded by the native SQLite driver.
#![cfg(feature = "native")]

use besu::sql;
use besu_sqlite::Native;
use chrono::{DateTime, Utc};

#[tokio::test]
async fn fractional_julian_dates_keep_their_side_of_the_unix_epoch() {
    let db = Native::in_memory().expect("open SQLite").database();
    let rows = db
        .execute(sql!(
            "SELECT julianday('1969-12-31 23:59:59.500'), \
                    julianday('1970-01-01 00:00:00.500')"
        ))
        .all::<(DateTime<Utc>, DateTime<Utc>)>()
        .await
        .expect("decode Julian dates");
    let (before, after) = rows[0];

    // A contemporary Julian day stored as REAL has about 40 µs of precision.
    assert!(
        (-501..=-499).contains(&before.timestamp_millis()),
        "{before}"
    );
    assert!((499..=501).contains(&after.timestamp_millis()), "{after}");
}
