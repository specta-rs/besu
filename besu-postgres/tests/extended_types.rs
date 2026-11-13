//! Round-trip the types that arrived after the primitives: interval, network addresses and
//! bit strings, plus a domain whose value is read through its base type.
#![cfg(feature = "sqlx")]
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

mod common;

use besu::prelude::*;
use besu::{BitString, Cidr, Inet, SqlInterval};
use besu_postgres::Sqlx;

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "extended")]
pub struct Extended {
    #[besu(primary_key)]
    pub id: i64,
    pub span: SqlInterval,
    pub host: Inet,
    pub network: Cidr,
    #[besu(sql_type = "besu::Bit<8>")]
    pub flags: BitString,
    #[besu(sql_type = "besu::VarBit<12>")]
    pub tail: BitString,
    pub maybe_span: Option<SqlInterval>,
}

impl Extended {
    fn sample(id: i64) -> Self {
        Self {
            id,
            // A month is not thirty days, so the two fields must survive separately.
            span: SqlInterval::new(14, 3, 4 * 3_600_000_000 + 5 * 60_000_000 + 6_789_000),
            host: Inet::new("192.168.0.1/24").unwrap(),
            network: Cidr::new("10.0.0.0/8").unwrap(),
            flags: BitString::new("10110011").unwrap(),
            tail: BitString::new("101010101010").unwrap(),
            maybe_span: None,
        }
    }
}

#[tokio::test]
async fn interval_network_and_bit_types_round_trip() {
    let db = common::database().await;
    common::reset(&db).await;
    db.push([extended.def()]).await.unwrap();

    // Every declared type maps onto a native type introspection reports back, so a second
    // push changes nothing.
    let second = db.push([extended.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);

    let original = Extended::sample(1);
    db.insert(extended)
        .value(extended.insert(
            original.id,
            original.span,
            original.host.clone(),
            original.network.clone(),
            original.flags.clone(),
            original.tail.clone(),
        ))
        .await
        .unwrap();

    let stored: Vec<Extended> = db.select(extended).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0], original);
}

#[tokio::test]
async fn a_nullable_interval_stays_null() {
    let db = common::database().await;
    common::reset(&db).await;
    db.push([extended.def()]).await.unwrap();

    let original = Extended::sample(2);
    db.insert(extended)
        .value(extended.insert(
            original.id,
            original.span,
            original.host.clone(),
            original.network.clone(),
            original.flags.clone(),
            original.tail.clone(),
        ))
        .await
        .unwrap();

    let stored: Vec<Extended> = db.select(extended).await.unwrap();
    assert_eq!(stored[0].maybe_span, None);
}

/// The declared types are visible in the schema before anything reaches the database.
#[tokio::test]
async fn the_declared_types_are_in_the_schema() {
    let schema = besu::Database::<Sqlx>::schema([extended.def()]);
    let table = schema.table("extended").unwrap();

    assert_eq!(table.column("span").unwrap().ty, TypeDef::Interval);
    assert_eq!(table.column("host").unwrap().ty, TypeDef::Inet);
    assert_eq!(table.column("network").unwrap().ty, TypeDef::Cidr);
    assert_eq!(table.column("flags").unwrap().ty, TypeDef::Bit(8));
    assert_eq!(table.column("tail").unwrap().ty, TypeDef::VarBit(12));
    assert_eq!(table.column("maybe_span").unwrap().ty, TypeDef::Interval);
    assert!(table.column("maybe_span").unwrap().nullable);
}

#[tokio::test]
async fn the_native_types_are_what_introspection_reports() {
    let db = common::database().await;
    common::reset(&db).await;
    db.push([extended.def()]).await.unwrap();

    let introspected = db.introspect().await.unwrap();
    let table = introspected.table("extended").unwrap();
    assert_eq!(table.column("span").unwrap().ty, TypeDef::Interval);
    assert_eq!(table.column("host").unwrap().ty, TypeDef::Inet);
    assert_eq!(table.column("network").unwrap().ty, TypeDef::Cidr);
    assert_eq!(table.column("flags").unwrap().ty, TypeDef::Bit(8));
    assert_eq!(table.column("tail").unwrap().ty, TypeDef::VarBit(12));

    let pushed = db.push_schema(&introspected).await.unwrap();
    assert!(pushed.is_empty(), "got {:?}", pushed.changes);
}

#[tokio::test]
async fn an_interval_keeps_its_three_fields() {
    let db = common::database().await;
    common::reset(&db).await;
    db.push([extended.def()]).await.unwrap();

    let values = [
        SqlInterval::zero(),
        SqlInterval::new(1, 0, 0),
        SqlInterval::new(0, 30, 0),
        SqlInterval::new(0, 0, -1),
        SqlInterval::new(-2, 3, i64::from(i32::MIN)),
    ];
    for (index, span) in values.into_iter().enumerate() {
        let id = index as i64;
        db.insert(extended)
            .value(extended.insert(
                id,
                span,
                Inet::new("::1").unwrap(),
                Cidr::new("0.0.0.0/0").unwrap(),
                BitString::new("00000001").unwrap(),
                BitString::new("1").unwrap(),
            ))
            .await
            .unwrap();

        let stored: Vec<Extended> = db
            .select(extended)
            .filter(extended.id.eq(id))
            .await
            .unwrap();
        assert_eq!(stored[0].span, span);
    }
}

/// Arrays of the new types travel through PostgreSQL's binary array format, whose header
/// names the element's OID. The element encoders are the same ones a scalar uses.
#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "extended_arrays")]
pub struct ExtendedArrays {
    #[besu(primary_key)]
    pub id: i64,
    pub spans: Vec<SqlInterval>,
    pub hosts: Vec<Inet>,
    pub networks: Vec<Cidr>,
}

#[tokio::test]
async fn arrays_of_interval_and_addresses_round_trip() {
    let db = common::database().await;
    common::reset(&db).await;
    db.push([extended_arrays.def()]).await.unwrap();

    let original = ExtendedArrays {
        id: 1,
        spans: vec![SqlInterval::new(0, 1, 0), SqlInterval::new(2, 0, 1)],
        hosts: vec![
            Inet::new("10.0.0.1").unwrap(),
            Inet::new("2001:db8::1/64").unwrap(),
        ],
        networks: vec![Cidr::new("192.168.1.0/24").unwrap()],
    };
    db.insert(extended_arrays)
        .value(extended_arrays.insert(
            original.id,
            original.spans.clone(),
            original.hosts.clone(),
            original.networks.clone(),
        ))
        .await
        .unwrap();

    let stored: Vec<ExtendedArrays> = db.select(extended_arrays).await.unwrap();
    assert_eq!(stored, [original]);

    let introspected = db.introspect().await.unwrap();
    let table = introspected.table("extended_arrays").unwrap();
    match &table.column("spans").unwrap().ty {
        TypeDef::Array(inner) => assert_eq!(inner.as_type(), &TypeDef::Interval),
        other => panic!("expected an interval array, got {other:?}"),
    }
    assert!(db.push_schema(&introspected).await.unwrap().is_empty());
}

/// A domain is a named type whose values are the base type's values. The column keeps the
/// domain's name, and the value decodes through the base type.
#[tokio::test]
async fn a_domain_keeps_its_name_and_decodes_through_its_base() {
    let db = common::database().await;
    common::reset(&db).await;

    db.execute(besu::Fragment::new(
        "CREATE DOMAIN positive_integer AS integer CHECK (VALUE > 0)".to_owned(),
    ))
    .await
    .unwrap();
    db.execute(besu::Fragment::new(
        "CREATE DOMAIN varchar_domain AS text".to_owned(),
    ))
    .await
    .unwrap();
    db.execute(besu::Fragment::new(
        "CREATE TABLE domain_holder (value positive_integer NOT NULL, \
         label varchar_domain NOT NULL)"
            .to_owned(),
    ))
    .await
    .unwrap();

    let introspected = db.introspect().await.unwrap();
    let table = introspected.table("domain_holder").unwrap();
    assert_eq!(
        table.column("value").unwrap().ty,
        TypeDef::Domain(besu::DomainDef::new("positive_integer", &TypeDef::Int4))
    );
    assert_eq!(
        table.column("label").unwrap().ty,
        TypeDef::Domain(besu::DomainDef::new("varchar_domain", &TypeDef::Text))
    );

    // The type is modelled, so a snapshot carries it and replays without a change.
    let pushed = db.push_schema(&introspected).await.unwrap();
    assert!(pushed.is_empty(), "got {:?}", pushed.changes);

    // A value written through the base type and read back through it.
    db.execute(besu::Fragment::new(
        "INSERT INTO domain_holder (value, label) VALUES (7, 'seven')".to_owned(),
    ))
    .await
    .unwrap();
    let value: i32 = db
        .execute(sql!("SELECT value FROM domain_holder"))
        .all::<i32>()
        .await
        .unwrap()
        .remove(0);
    assert_eq!(value, 7);
    let label: String = db
        .execute(sql!("SELECT label FROM domain_holder"))
        .all::<String>()
        .await
        .unwrap()
        .remove(0);
    assert_eq!(label, "seven");
    // A non-numeric domain value does not pretend to be a number.
    assert!(
        db.execute(sql!("SELECT label FROM domain_holder"))
            .all::<i32>()
            .await
            .is_err()
    );

    db.execute(besu::Fragment::new("DROP TABLE domain_holder".to_owned()))
        .await
        .unwrap();
    db.execute(besu::Fragment::new(
        "DROP DOMAIN positive_integer CASCADE".to_owned(),
    ))
    .await
    .unwrap();
    db.execute(besu::Fragment::new(
        "DROP DOMAIN varchar_domain CASCADE".to_owned(),
    ))
    .await
    .unwrap();
}

/// A snapshot that declares a domain creates the type before the table that uses it, so
/// pushing it into an empty database works.
#[tokio::test]
async fn a_domain_snapshot_creates_the_type() {
    let db = common::database().await;
    common::reset(&db).await;

    db.execute(besu::Fragment::new(
        "CREATE DOMAIN public.price AS numeric(10,2)".to_owned(),
    ))
    .await
    .unwrap();
    db.execute(besu::Fragment::new(
        "CREATE TABLE priced (amount price NOT NULL)".to_owned(),
    ))
    .await
    .unwrap();
    let snapshot = db.introspect().await.unwrap();
    assert_eq!(snapshot.domains.len(), 1);
    assert_eq!(snapshot.domains[0].name, "price");
    assert_eq!(snapshot.domains[0].base, TypeDef::Decimal(10, 2));

    common::reset(&db).await;
    let pushed = db.push_schema(&snapshot).await.unwrap();
    assert!(
        pushed
            .changes
            .iter()
            .any(|change| matches!(change, besu::schema::Change::CreateDomain(domain) if domain.name == "price")),
        "got {:?}",
        pushed.changes
    );

    let stored = db.introspect().await.unwrap();
    assert_eq!(stored.domains.len(), 1);
    let table = stored.table("priced").unwrap();
    assert_eq!(
        table.column("amount").unwrap().ty,
        TypeDef::Domain(besu::DomainDef::new("price", &TypeDef::Decimal(10, 2)))
    );
    assert!(db.push_schema(&stored).await.unwrap().is_empty());

    db.execute(besu::Fragment::new("DROP TABLE priced".to_owned()))
        .await
        .unwrap();
    db.execute(besu::Fragment::new("DROP DOMAIN price CASCADE".to_owned()))
        .await
        .unwrap();
}
