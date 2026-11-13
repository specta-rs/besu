//! MySQL's spatial types, and its text storage for ranges and vectors.
#![allow(
    missing_docs,
    clippy::panic,
    clippy::too_many_arguments,
    clippy::unwrap_used
)]

use besu::prelude::*;
use besu::{Geometry, Range, TypeDef, Vector};
use besu_mysql::Sqlx;
use sqlx::MySqlPool;

/// The MySQL server the suite runs against.
const SERVER: &str = "mysql://root:besu@localhost:53306";

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "places")]
pub struct Place {
    #[besu(primary_key)]
    pub id: i64,
    pub any: Geometry,
    #[besu(sql_type = "besu::Point")]
    pub point: Geometry,
    #[besu(sql_type = "besu::Point<4326>")]
    pub located: Geometry,
    #[besu(sql_type = "besu::LineString<4326>")]
    pub line: Geometry,
    #[besu(sql_type = "besu::Polygon")]
    pub area: Geometry,
    #[besu(sql_type = "besu::GeometryCollection<4326>")]
    pub collection: Geometry,
    // MySQL has no range type: the canonical text is stored in a `TEXT` column.
    pub window: Range<i32>,
    // MySQL 8.4 has no vector type either: the text is stored in a `TEXT` column.
    #[besu(sql_type = "besu::VectorOf<3>")]
    pub embedding: Vector,
}

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

fn point(srid: bool) -> Geometry {
    if srid {
        Geometry::from_hex("0101000020E6100000000000000000F03F0000000000000040").unwrap()
    } else {
        Geometry::from_hex("0101000000000000000000F03F0000000000000040").unwrap()
    }
}

fn line(srid: bool) -> Geometry {
    let mut hex = String::from("010200");
    if srid {
        hex.push_str("0020E6100000");
    } else {
        hex.push_str("0000");
    }
    hex.push_str("02000000");
    hex.push_str("00000000000000000000000000000000");
    hex.push_str("000000000000F03F000000000000F03F");
    Geometry::from_hex(&hex).unwrap()
}

fn polygon() -> Geometry {
    Geometry::from_hex(concat!(
        "01030000000100000004000000",
        "00000000000000000000000000000000",
        "000000000000F03F0000000000000000",
        "000000000000F03F000000000000F03F",
        "00000000000000000000000000000000",
    ))
    .unwrap()
}

fn collection(srid: bool) -> Geometry {
    let mut hex = String::from("010700");
    if srid {
        hex.push_str("0020E6100000");
    } else {
        hex.push_str("0000");
    }
    hex.push_str("01000000");
    // One child point, as plain WKB.
    hex.push_str("0101000000000000000000F03F0000000000000040");
    Geometry::from_hex(&hex).unwrap()
}

impl Place {
    fn sample(id: i64) -> Self {
        Self {
            id,
            any: point(true),
            point: point(false),
            located: point(true),
            line: line(true),
            area: polygon(),
            collection: collection(true),
            // MySQL has no range semantics, so the bounds and their inclusion round-trip
            // exactly as they were written.
            window: Range::closed(1, 5),
            embedding: Vector::new(vec![1.0, 2.5, -3.0]),
        }
    }
}

#[tokio::test]
async fn spatial_types_ranges_and_vectors_round_trip() {
    let db = fresh("besu_spatial").await;
    db.push([places.def()]).await.unwrap();

    let second = db.push([places.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);

    let original = Place::sample(1);
    db.insert(places)
        .value(places.insert(
            original.id,
            original.any.clone(),
            original.point.clone(),
            original.located.clone(),
            original.line.clone(),
            original.area.clone(),
            original.collection.clone(),
            original.window,
            original.embedding.clone(),
        ))
        .await
        .unwrap();

    let stored: Vec<Place> = db.select(places).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0], original);
    assert_eq!(stored[0].located.srid(), Some(4326));
    assert_eq!(stored[0].area.kind(), Some(besu::GeometryKind::Polygon));
    // MySQL has no range semantics, so an inclusive bound stays inclusive: `[1,5]` is not
    // canonicalised to `[1,6)` the way PostgreSQL does for a discrete range.
    assert_eq!(stored[0].window, Range::closed(1, 5));

    let introspected = db.introspect().await.unwrap();
    let table = introspected.table("places").unwrap();
    assert_eq!(
        table.column("any").unwrap().ty,
        TypeDef::Geometry {
            kind: besu::GeometryKind::Geometry,
            srid: None,
        }
    );
    assert_eq!(
        table.column("point").unwrap().ty,
        TypeDef::Geometry {
            kind: besu::GeometryKind::Point,
            srid: None,
        }
    );
    assert_eq!(
        table.column("located").unwrap().ty,
        TypeDef::Geometry {
            kind: besu::GeometryKind::Point,
            srid: Some(4326),
        }
    );
    assert_eq!(
        table.column("line").unwrap().ty,
        TypeDef::Geometry {
            kind: besu::GeometryKind::LineString,
            srid: Some(4326),
        }
    );
    assert_eq!(
        table.column("area").unwrap().ty,
        TypeDef::Geometry {
            kind: besu::GeometryKind::Polygon,
            srid: None,
        }
    );
    assert_eq!(
        table.column("collection").unwrap().ty,
        TypeDef::Geometry {
            kind: besu::GeometryKind::GeometryCollection,
            srid: Some(4326),
        }
    );
    // The fallbacks: a range and a vector are both plain `TEXT` on MySQL.
    assert_eq!(table.column("window").unwrap().ty, TypeDef::Text);
    assert_eq!(table.column("embedding").unwrap().ty, TypeDef::Text);

    // Nothing the data or the introspection saw is drift, so a third push is still empty.
    let third = db.push([places.def()]).await.unwrap();
    assert!(third.is_empty(), "got {:?}", third.changes);
}
