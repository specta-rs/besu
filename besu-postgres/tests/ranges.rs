//! PostgreSQL's six built-in range types: the binary format, the text form, arrays of
//! ranges, introspection and `push` convergence.
#![cfg(feature = "sqlx")]
#![allow(
    missing_docs,
    clippy::panic,
    clippy::too_many_arguments,
    clippy::unwrap_used
)]

mod common;

use besu::prelude::*;
use besu::{Bound, Range, TypeDef};
use besu_postgres::Sqlx;

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "ranges")]
pub struct Ranges {
    #[besu(primary_key)]
    pub id: i64,
    pub ints: Range<i32>,
    pub bigs: Range<i64>,
    // `numrange`'s subtype is an unconstrained `numeric`, so the precision and scale are
    // not part of the column type; `Decimal<0, 0>` is the spelling that converges.
    #[besu(sql_type = "besu::Range<besu::Decimal<0, 0>>")]
    pub nums: Range<String>,
    pub times: Range<chrono::NaiveDateTime>,
    pub stamps: Range<chrono::DateTime<chrono::Utc>>,
    pub dates: Range<chrono::NaiveDate>,
    pub maybe: Option<Range<i32>>,
    pub spans: Vec<Range<i32>>,
}

impl Ranges {
    fn sample(id: i64) -> Self {
        Self {
            id,
            // PostgreSQL canonicalises a discrete range, so `[1,5]` is stored as `[1,6)` and
            // `(x,y)` as `[x+1,y)`. The sample uses the canonical spellings so the model that
            // comes back compares equal.
            ints: Range::closed_open(1, 6),
            bigs: Range::closed_open((1_i64 << 40) + 1, (1_i64 << 40) + 10),
            nums: Range::closed_open("1.25".to_owned(), "9".to_owned()),
            times: Range::closed(
                chrono::NaiveDate::from_ymd_opt(2020, 1, 1)
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap(),
                chrono::NaiveDate::from_ymd_opt(2020, 2, 1)
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap(),
            ),
            stamps: Range::new(
                Bound::Included(chrono::DateTime::from_timestamp(1_600_000_000, 0).unwrap()),
                Bound::Excluded(chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap()),
            ),
            dates: Range::new(
                Bound::Included(chrono::NaiveDate::from_ymd_opt(2020, 1, 1).unwrap()),
                Bound::Unbounded,
            ),
            maybe: None,
            spans: vec![
                Range::closed_open(1, 3),
                Range::empty(),
                Range::new(Bound::Unbounded, Bound::Excluded(10)),
            ],
        }
    }
}

#[tokio::test]
async fn every_range_type_round_trips_and_converges() {
    let db = common::database().await;
    common::reset(&db).await;
    db.push([ranges.def()]).await.unwrap();

    // Every declared type maps onto a native range type introspection reports back, so a
    // second push changes nothing.
    let second = db.push([ranges.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);

    let original = Ranges::sample(1);
    db.insert(ranges)
        .value(
            ranges
                .insert(
                    original.id,
                    original.ints,
                    original.bigs,
                    original.nums.clone(),
                    original.times,
                    original.stamps,
                    original.dates,
                    original.spans.clone(),
                )
                .maybe(original.maybe),
        )
        .await
        .unwrap();

    let stored: Vec<Ranges> = db.select(ranges).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0], original);
}

#[tokio::test]
async fn an_empty_and_an_unbounded_range_survive_the_wire() {
    let db = common::database().await;
    common::reset(&db).await;
    db.push([ranges.def()]).await.unwrap();

    // The empty range is a value of its own, not a range with no bounds: both ends of `(,)`
    // are infinite, while `empty` contains nothing at all.
    let original = Ranges {
        ints: Range::empty(),
        bigs: Range::new(Bound::<i64>::Unbounded, Bound::Unbounded),
        spans: Vec::new(),
        ..Ranges::sample(2)
    };
    db.insert(ranges)
        .value(
            ranges
                .insert(
                    original.id,
                    original.ints,
                    original.bigs,
                    original.nums.clone(),
                    original.times,
                    original.stamps,
                    original.dates,
                    original.spans.clone(),
                )
                .maybe(original.maybe),
        )
        .await
        .unwrap();

    let stored: Vec<Ranges> = db.select(ranges).await.unwrap();
    assert_eq!(stored[0].ints, Range::empty());
    assert_eq!(
        stored[0].bigs,
        Range::new(Bound::Unbounded, Bound::Unbounded)
    );
    assert!(stored[0].spans.is_empty());

    let third = db.push([ranges.def()]).await.unwrap();
    assert!(third.is_empty(), "got {:?}", third.changes);
    let introspected = db.introspect().await.unwrap();
    let table = introspected.table("ranges").unwrap();
    assert_eq!(
        table.column("ints").unwrap().ty,
        TypeDef::Range(besu_core::types::TypeRef::owned(TypeDef::Int4))
    );
}

#[tokio::test]
async fn introspection_reports_each_range_subtype() {
    let db = common::database().await;
    common::reset(&db).await;
    db.push([ranges.def()]).await.unwrap();

    let introspected = db.introspect().await.unwrap();
    let table = introspected.table("ranges").unwrap();
    assert_eq!(
        table.column("ints").unwrap().ty,
        TypeDef::Range(besu_core::types::TypeRef::owned(TypeDef::Int4))
    );
    assert_eq!(
        table.column("bigs").unwrap().ty,
        TypeDef::Range(besu_core::types::TypeRef::owned(TypeDef::Int8))
    );
    assert_eq!(
        table.column("nums").unwrap().ty,
        TypeDef::Range(besu_core::types::TypeRef::owned(TypeDef::Decimal(0, 0)))
    );
    assert_eq!(
        table.column("times").unwrap().ty,
        TypeDef::Range(besu_core::types::TypeRef::owned(TypeDef::Timestamp))
    );
    assert_eq!(
        table.column("stamps").unwrap().ty,
        TypeDef::Range(besu_core::types::TypeRef::owned(TypeDef::TimestampTz))
    );
    assert_eq!(
        table.column("dates").unwrap().ty,
        TypeDef::Range(besu_core::types::TypeRef::owned(TypeDef::Date))
    );
    // An array of ranges is a real array type whose element is the range.
    assert_eq!(
        table.column("spans").unwrap().ty,
        TypeDef::Array(besu_core::types::TypeRef::owned(TypeDef::Range(
            besu_core::types::TypeRef::owned(TypeDef::Int4)
        )))
    );
}

#[tokio::test]
async fn the_declared_range_types_are_in_the_schema() {
    let schema = besu::Database::<Sqlx>::schema([ranges.def()]);
    let table = schema.table("ranges").unwrap();
    assert_eq!(table.column("ints").unwrap().ty, {
        TypeDef::Range(besu_core::types::TypeRef::owned(TypeDef::Int4))
    });
    assert_eq!(table.column("maybe").unwrap().ty, {
        TypeDef::Range(besu_core::types::TypeRef::owned(TypeDef::Int4))
    });
    assert!(table.column("maybe").unwrap().nullable);
}
