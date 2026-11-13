//! SQLite has no interval, address or bit-string type, so each is stored as its canonical
//! text. This pins the fallback down: the declaration renders `TEXT`, a value round-trips,
//! and the column converges against what `PRAGMA table_info` reports.
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::prelude::*;
use besu::{BitString, Cidr, Inet, SqlInterval, TypeDef};
use besu_sqlite::Sqlx;

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "extended")]
pub struct Extended {
    #[besu(primary_key)]
    pub id: i64,
    pub span: SqlInterval,
    pub host: Inet,
    pub network: Cidr,
    #[besu(sql_type = "besu::Bit<5>")]
    pub flags: BitString,
    #[besu(sql_type = "besu::VarBit<0>")]
    pub tail: BitString,
}

#[tokio::test]
async fn the_text_fallbacks_round_trip() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.push([extended.def()]).await.unwrap();

    // Every one of them is `TEXT` here, so a second push is a no-op.
    let second = db.push([extended.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);

    let original = Extended {
        id: 1,
        span: SqlInterval::new(2, 3, 4 * 3_600_000_000 + 5 * 60_000_000),
        host: Inet::new("10.0.0.1/8").unwrap(),
        network: Cidr::new("192.168.1.0/24").unwrap(),
        flags: BitString::new("10110").unwrap(),
        tail: BitString::new("0011").unwrap(),
    };
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
    assert_eq!(stored, [original]);
}

#[tokio::test]
async fn the_declared_and_introspected_types_fold_onto_text() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.push([extended.def()]).await.unwrap();

    let declared = besu::Database::<Sqlx>::schema([extended.def()]);
    let table = declared.table("extended").unwrap();
    assert_eq!(table.column("span").unwrap().ty, TypeDef::Interval);
    assert_eq!(table.column("host").unwrap().ty, TypeDef::Inet);
    assert_eq!(table.column("network").unwrap().ty, TypeDef::Cidr);
    assert_eq!(table.column("flags").unwrap().ty, TypeDef::Bit(5));

    let introspected = db.introspect().await.unwrap();
    let table = introspected.table("extended").unwrap();
    for column in ["span", "host", "network", "flags", "tail"] {
        assert_eq!(
            table.column(column).unwrap().ty,
            TypeDef::Text,
            "{column} must be text"
        );
    }

    let pushed = db.push_schema(&introspected).await.unwrap();
    assert!(pushed.is_empty(), "got {:?}", pushed.changes);
}

#[tokio::test]
async fn a_value_written_by_hand_parses_back() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.push([extended.def()]).await.unwrap();

    // The stored spelling is the one `Display` writes, so a hand-written row of the same
    // shape decodes like one the driver wrote.
    db.execute(besu::Fragment::new(
        "INSERT INTO extended (id, span, host, network, flags, tail) \
         VALUES (1, '1 days 02:00:00', '192.168.0.1', '10.0.0.0/8', '10110', '1')"
            .to_owned(),
    ))
    .await
    .unwrap();

    let stored: Vec<Extended> = db.select(extended).await.unwrap();
    assert_eq!(stored[0].span, SqlInterval::new(0, 1, 2 * 3_600_000_000));
    assert_eq!(stored[0].host, Inet::new("192.168.0.1").unwrap());
    assert_eq!(stored[0].network, Cidr::new("10.0.0.0/8").unwrap());
    assert_eq!(stored[0].flags.as_str(), "10110");
    assert_eq!(stored[0].tail.as_str(), "1");
}
