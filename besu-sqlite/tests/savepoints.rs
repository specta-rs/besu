//! Nested transactions, which every supported database spells as savepoints.
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::prelude::*;
use besu_sqlite::Sqlx;

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "notes")]
pub struct Note {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub body: String,
}

async fn db() -> besu::Database<Sqlx> {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.push([notes.def()]).await.unwrap();
    db
}

async fn count(db: &besu::Database<Sqlx>) -> i64 {
    db.select(notes).pick(count_all()).await.unwrap().remove(0)
}

#[tokio::test]
async fn a_failed_savepoint_leaves_the_outer_work_intact() {
    let db = db().await;

    db.transaction(async |tx| {
        tx.insert(notes).value(notes.insert("outer")).await?;

        // The inner block fails, and only what it did is undone.
        let inner: Result<(), _> = tx
            .transaction(async |tx| {
                tx.insert(notes).value(notes.insert("inner")).await?;
                Err::<(), _>(sqlx::Error::Protocol("deliberate".into()))
            })
            .await;
        assert!(inner.is_err());

        // And now the outer transaction fails too, so everything goes.
        Err::<(), _>(sqlx::Error::Protocol("outer".into()))
    })
    .await
    .unwrap_err();

    // The whole outer transaction still rolled back, because it returned an error.
    assert_eq!(count(&db).await, 0);
}

#[tokio::test]
async fn a_failed_savepoint_keeps_the_outer_insert() {
    let db = db().await;

    db.transaction(async |tx| {
        tx.insert(notes).value(notes.insert("outer")).await?;

        // Swallow the inner failure: the savepoint rolls back, the outer insert stays.
        let _ = tx
            .transaction(async |tx| {
                tx.insert(notes).value(notes.insert("inner")).await?;
                Err::<(), _>(sqlx::Error::Protocol("deliberate".into()))
            })
            .await;

        Ok::<(), sqlx::Error>(())
    })
    .await
    .unwrap();

    let bodies: Vec<String> = db
        .select(notes)
        .pick(notes.body)
        .order_by(notes.id.asc())
        .await
        .unwrap();
    assert_eq!(bodies, ["outer"]);
}

#[tokio::test]
async fn a_successful_savepoint_releases_its_rows() {
    let db = db().await;

    db.transaction(async |tx| {
        tx.transaction(async |tx| {
            tx.insert(notes).value(notes.insert("inner")).await?;
            Ok::<(), sqlx::Error>(())
        })
        .await?;
        Ok::<(), sqlx::Error>(())
    })
    .await
    .unwrap();

    assert_eq!(count(&db).await, 1);
}

#[tokio::test]
async fn savepoints_can_be_named_and_nest() {
    let db = db().await;

    db.transaction(async |tx| {
        tx.savepoint("first", async |tx| {
            tx.insert(notes).value(notes.insert("one")).await?;
            tx.savepoint("second", async |tx| {
                tx.insert(notes).value(notes.insert("two")).await?;
                Ok::<(), sqlx::Error>(())
            })
            .await?;
            Ok::<(), sqlx::Error>(())
        })
        .await?;
        Ok::<(), sqlx::Error>(())
    })
    .await
    .unwrap();

    assert_eq!(count(&db).await, 2);
}

#[tokio::test]
async fn a_savepoint_name_cannot_carry_sql() {
    let db = db().await;

    let error = db
        .transaction(async |tx| {
            tx.savepoint("x; DROP TABLE notes", async |_| Ok::<(), sqlx::Error>(()))
                .await
        })
        .await
        .unwrap_err();

    assert!(error.to_string().contains("savepoint name"), "got: {error}");
    // The table is still there.
    assert_eq!(count(&db).await, 0);
}
