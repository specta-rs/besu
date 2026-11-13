//! PostGIS and `pgvector` columns. Both are extension types, so each test skips (with a
//! message) when the server does not have the extension installed.
#![allow(
    missing_docs,
    clippy::panic,
    clippy::too_many_arguments,
    clippy::unwrap_used
)]

mod common;

use besu::prelude::*;
use besu::{Geometry, TypeDef, Vector};
use besu_postgres::Sqlx;

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "spatial")]
pub struct Spatial {
    #[besu(primary_key)]
    pub id: i64,
    // `GEOMETRY` accepts every shape and has no SRID of its own.
    pub any: Geometry,
    #[besu(sql_type = "besu::Point<4326>")]
    pub point: Geometry,
    #[besu(sql_type = "besu::LineString<4326>")]
    pub line: Geometry,
    #[besu(sql_type = "besu::GeographyPoint<4326>")]
    pub area: Geometry,
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "embeddings")]
pub struct Embeddings {
    #[besu(primary_key)]
    pub id: i64,
    #[besu(sql_type = "besu::VectorOf<3>")]
    pub xyz: Vector,
    // A `vector` with no dimension accepts any number of components.
    pub any: Vector,
}

/// Install an extension, reporting whether the server has it.
async fn extension(db: &besu::Database<Sqlx>, name: &str) -> bool {
    let statement = format!("CREATE EXTENSION IF NOT EXISTS {name}");
    match db.execute(besu::Fragment::new(statement)).await {
        Ok(_) => true,
        Err(_) => {
            eprintln!("skipping: the server does not have the `{name}` extension");
            false
        }
    }
}

/// The EWKB of `POINT(1 2)` with SRID 4326.
fn point() -> Geometry {
    Geometry::from_hex("0101000020E6100000000000000000F03F0000000000000040").unwrap()
}

/// The EWKB of `LINESTRING(0 0,1 1)` with SRID 4326.
fn line() -> Geometry {
    Geometry::from_hex(
        "0102000020E61000000200000000000000000000000000000000000000000000000000F03F000000000000F03F",
    )
    .unwrap()
}

#[tokio::test]
async fn postgis_geometry_and_geography_round_trip() {
    let db = common::database().await;
    if !extension(&db, "postgis").await {
        return;
    }
    common::reset(&db).await;
    db.push([spatial.def()]).await.unwrap();

    let second = db.push([spatial.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);

    let original = Spatial {
        id: 1,
        any: point(),
        point: point(),
        line: line(),
        area: point(),
    };
    db.insert(spatial)
        .value(spatial.insert(
            original.id,
            original.any.clone(),
            original.point.clone(),
            original.line.clone(),
            original.area.clone(),
        ))
        .await
        .unwrap();

    let stored: Vec<Spatial> = db.select(spatial).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0], original);
    assert_eq!(stored[0].point.srid(), Some(4326));
    assert_eq!(stored[0].point.kind(), Some(besu::GeometryKind::Point));
    assert_eq!(stored[0].line.kind(), Some(besu::GeometryKind::LineString));

    let introspected = db.introspect().await.unwrap();
    let table = introspected.table("spatial").unwrap();
    assert_eq!(
        table.column("point").unwrap().ty,
        TypeDef::Geometry {
            kind: besu::GeometryKind::Point,
            srid: Some(4326),
        }
    );
    assert_eq!(
        table.column("any").unwrap().ty,
        TypeDef::Geometry {
            kind: besu::GeometryKind::Geometry,
            srid: None,
        }
    );
    assert_eq!(
        table.column("area").unwrap().ty,
        TypeDef::Geography {
            kind: besu::GeometryKind::Point,
            srid: Some(4326),
        }
    );
}

#[tokio::test]
async fn pgvector_round_trips_and_converges() {
    let db = common::database().await;
    if !extension(&db, "vector").await {
        return;
    }
    common::reset(&db).await;
    db.push([embeddings.def()]).await.unwrap();

    let second = db.push([embeddings.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);

    let original = Embeddings {
        id: 1,
        xyz: Vector::new(vec![1.0, 2.5, -3.0]),
        any: Vector::new(vec![0.5, 1.5, 2.5, 3.5]),
    };
    db.insert(embeddings)
        .value(embeddings.insert(original.id, original.xyz.clone(), original.any.clone()))
        .await
        .unwrap();

    let stored: Vec<Embeddings> = db.select(embeddings).await.unwrap();
    assert_eq!(stored[0], original);

    let introspected = db.introspect().await.unwrap();
    let table = introspected.table("embeddings").unwrap();
    assert_eq!(table.column("xyz").unwrap().ty, TypeDef::Vector(3));
    assert_eq!(table.column("any").unwrap().ty, TypeDef::Vector(0));
}
