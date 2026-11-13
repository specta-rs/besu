//! SQLite has no range, spatial or vector type. A range and a vector are stored in `TEXT`,
//! a spatial value in a `BLOB`, and the declaration folds onto that storage so `push`
//! converges. This pins the fallback down.
#![cfg(feature = "sqlx")]
#![allow(
    missing_docs,
    clippy::panic,
    clippy::too_many_arguments,
    clippy::unwrap_used
)]

use besu::prelude::*;
use besu::{Bound, Geometry, Range, TypeDef, Vector};
use besu_sqlite::Sqlx;

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "fallbacks")]
pub struct Fallbacks {
    #[besu(primary_key)]
    pub id: i64,
    pub window: Range<i32>,
    pub day: Range<chrono::NaiveDate>,
    pub any: Geometry,
    #[besu(sql_type = "besu::Point<4326>")]
    pub point: Geometry,
    #[besu(sql_type = "besu::VectorOf<3>")]
    pub embedding: Vector,
}

#[tokio::test]
async fn the_fallback_storage_round_trips_and_converges() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.push([fallbacks.def()]).await.unwrap();

    let second = db.push([fallbacks.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);

    let point = Geometry::from_hex("0101000020E6100000000000000000F03F0000000000000040").unwrap();
    let original = Fallbacks {
        id: 1,
        // SQLite has no range semantics, so the bounds and their inclusion are exactly what
        // was written.
        window: Range::new(Bound::Included(1), Bound::Excluded(5)),
        day: Range::new(
            Bound::Included(chrono::NaiveDate::from_ymd_opt(2020, 1, 1).unwrap()),
            Bound::Unbounded,
        ),
        any: point.clone(),
        point,
        embedding: Vector::new(vec![1.0, 2.5, -3.0]),
    };
    db.insert(fallbacks)
        .value(fallbacks.insert(
            original.id,
            original.window,
            original.day,
            original.any.clone(),
            original.point.clone(),
            original.embedding.clone(),
        ))
        .await
        .unwrap();

    let stored: Vec<Fallbacks> = db.select(fallbacks).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0], original);
    assert_eq!(stored[0].point.srid(), Some(4326));
}

#[tokio::test]
async fn an_empty_range_survives_the_text_fallback() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.push([fallbacks.def()]).await.unwrap();

    let original = Fallbacks {
        id: 2,
        window: Range::empty(),
        day: Range::new(Bound::<chrono::NaiveDate>::Unbounded, Bound::Unbounded),
        any: Geometry::from_hex("0101000000000000000000F03F0000000000000040").unwrap(),
        point: Geometry::from_hex("0101000020E6100000000000000000F03F0000000000000040").unwrap(),
        embedding: Vector::default(),
    };
    db.insert(fallbacks)
        .value(fallbacks.insert(
            original.id,
            original.window,
            original.day,
            original.any.clone(),
            original.point.clone(),
            original.embedding.clone(),
        ))
        .await
        .unwrap();

    let stored: Vec<Fallbacks> = db.select(fallbacks).await.unwrap();
    assert_eq!(stored[0].window, Range::empty());
    assert_eq!(
        stored[0].day,
        Range::new(Bound::Unbounded, Bound::Unbounded)
    );
    assert!(stored[0].embedding.is_empty());

    let second = db.push([fallbacks.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);
    let introspected = db.introspect().await.unwrap();
    let table = introspected.table("fallbacks").unwrap();
    assert_eq!(table.column("window").unwrap().ty, TypeDef::Text);
    assert_eq!(table.column("any").unwrap().ty, TypeDef::Bytes);
}

#[tokio::test]
async fn introspection_reports_the_fallback_storage() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.push([fallbacks.def()]).await.unwrap();

    let introspected = db.introspect().await.unwrap();
    let table = introspected.table("fallbacks").unwrap();
    assert_eq!(table.column("window").unwrap().ty, TypeDef::Text);
    assert_eq!(table.column("day").unwrap().ty, TypeDef::Text);
    assert_eq!(table.column("any").unwrap().ty, TypeDef::Bytes);
    assert_eq!(table.column("point").unwrap().ty, TypeDef::Bytes);
    assert_eq!(table.column("embedding").unwrap().ty, TypeDef::Text);
}
