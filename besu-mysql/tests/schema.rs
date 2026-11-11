//! Schema fidelity on MySQL: a functional index is skipped rather than reported as a
//! smaller index, a view is reported but never touched, and introspection converges with
//! itself.
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::prelude::*;
use besu_mysql::Sqlx;
use sqlx::MySqlPool;

/// The MySQL server the suite runs against.
const SERVER: &str = "mysql://root:besu@localhost:53306";

/// Create an empty database named `name`, replacing any database left over from a previous
/// run.
async fn fresh(name: &str) -> besu::Database<Sqlx> {
    let admin = MySqlPool::connect(SERVER).await.unwrap();
    sqlx::query(&format!("DROP DATABASE IF EXISTS `{name}`"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("CREATE DATABASE `{name}`"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;

    let url = format!("{SERVER}/{name}");
    besu::Database::new(Sqlx::new(&url).await.unwrap())
}

/// A functional index has an expression where a column name would be. The IR cannot express
/// one, and reporting the remaining key parts would describe an index that does not exist,
/// so the whole index is skipped.
#[tokio::test]
async fn a_functional_index_is_skipped() {
    let db = fresh("besu_test_schema_functional_index").await;

    db.execute(sql!(
        "CREATE TABLE events (\
             id BIGINT PRIMARY KEY, \
             kind VARCHAR(50) NOT NULL, \
             payload VARCHAR(200) NOT NULL)"
    ))
    .await
    .unwrap();
    db.execute(sql!("CREATE INDEX events_kind_idx ON events (kind)"))
        .await
        .unwrap();
    db.execute(sql!(
        "CREATE INDEX events_lower_payload_idx ON events ((lower(payload)))"
    ))
    .await
    .unwrap();

    let introspected = db.introspect().await.unwrap();
    let events = introspected.table("events").unwrap();
    assert_eq!(
        events.indexes.len(),
        1,
        "the functional index must be skipped as a whole: {:?}",
        events.indexes
    );
    assert_eq!(events.indexes[0].name, "events_kind_idx");
    assert_eq!(events.indexes[0].columns, ["kind"]);

    // Pushing what introspection saw is a no-op: nothing is dropped or re-created.
    assert!(db.push_schema(&introspected).await.unwrap().is_empty());
}

/// A view is reported so `besu pull` writes it down, but the differ never creates or drops
/// one.
#[tokio::test]
async fn a_view_is_reported_but_never_touched() {
    let db = fresh("besu_test_schema_view").await;

    db.execute(sql!("CREATE TABLE users (id BIGINT PRIMARY KEY)"))
        .await
        .unwrap();
    db.execute(sql!("CREATE VIEW user_ids AS SELECT id FROM users"))
        .await
        .unwrap();

    let introspected = db.introspect().await.unwrap();
    assert!(
        introspected.table("user_ids").is_none(),
        "a view must not be reported as a table"
    );
    assert_eq!(introspected.views.len(), 1);
    assert_eq!(introspected.views[0].name, "user_ids");
    assert!(
        introspected.views[0].definition.contains("users"),
        "got {:?}",
        introspected.views[0].definition
    );

    let pushed = db.push_schema(&introspected).await.unwrap();
    assert!(pushed.is_empty(), "got {:?}", pushed.changes);
}
