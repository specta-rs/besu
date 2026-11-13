//! `#[derive(Table)]` maps the new value types onto their column types without an explicit
//! `sql_type`, and documents the places where one is needed.
#![allow(
    missing_docs,
    clippy::panic,
    clippy::too_many_arguments,
    clippy::unwrap_used
)]

use besu::prelude::*;
use besu::{Geometry, GeometryKind, Range, TypeDef, Vector};
use besu_core::types::TypeRef;

#[derive(Table)]
#[besu(table = "mapped")]
pub struct Mapped {
    #[besu(primary_key)]
    pub id: i64,
    pub window: Range<i32>,
    pub span: Range<i64>,
    pub seen: Range<chrono::NaiveDateTime>,
    pub recorded: Range<chrono::DateTime<chrono::Utc>>,
    pub day: Range<chrono::NaiveDate>,
    // The dimension and the shape are part of the declared type, so these need one.
    #[besu(sql_type = "besu::VectorOf<3>")]
    pub xyz: Vector,
    #[besu(sql_type = "besu::Point<4326>")]
    pub home: Geometry,
    #[besu(sql_type = "besu::GeographyPoint<4326>")]
    pub area: Geometry,
    #[besu(sql_type = "besu::Range<besu::Decimal<10, 2>>")]
    pub amount: Range<String>,
    pub spans: Vec<Range<i32>>,
}

#[test]
fn the_derive_infers_ranges_and_the_bare_value_types() {
    let def = mapped.def();
    let ty = |name: &str| def.column(name).unwrap().ty.clone();

    assert_eq!(ty("window"), TypeDef::Range(TypeRef::owned(TypeDef::Int4)));
    assert_eq!(ty("span"), TypeDef::Range(TypeRef::owned(TypeDef::Int8)));
    assert_eq!(
        ty("seen"),
        TypeDef::Range(TypeRef::owned(TypeDef::Timestamp))
    );
    assert_eq!(
        ty("recorded"),
        TypeDef::Range(TypeRef::owned(TypeDef::TimestampTz))
    );
    assert_eq!(ty("day"), TypeDef::Range(TypeRef::owned(TypeDef::Date)));
    assert_eq!(
        ty("xyz"),
        TypeDef::Vector(3),
        "the marker carries the dimension"
    );
    assert_eq!(
        ty("home"),
        TypeDef::Geometry {
            kind: GeometryKind::Point,
            srid: Some(4326),
        }
    );
    assert_eq!(
        ty("area"),
        TypeDef::Geography {
            kind: GeometryKind::Point,
            srid: Some(4326),
        }
    );
    assert_eq!(
        ty("amount"),
        TypeDef::Range(TypeRef::owned(TypeDef::Decimal(10, 2)))
    );
    // A `Vec` of a range is an array of the range type, exactly as a `Vec<i32>` is.
    assert_eq!(
        ty("spans"),
        TypeDef::Array(TypeRef::owned(TypeDef::Range(TypeRef::owned(
            TypeDef::Int4
        ))))
    );
}

#[derive(Table)]
#[besu(table = "bare")]
pub struct Bare {
    #[besu(primary_key)]
    pub id: i64,
    pub location: Geometry,
    pub embedding: Vector,
}

#[test]
fn a_bare_spatial_or_vector_field_is_the_unmodified_type() {
    let def = bare.def();
    assert_eq!(
        def.column("location").unwrap().ty,
        TypeDef::Geometry {
            kind: GeometryKind::Geometry,
            srid: None,
        }
    );
    assert_eq!(def.column("embedding").unwrap().ty, TypeDef::Vector(0));
}
