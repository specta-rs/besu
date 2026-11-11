//! Round-trip the types that are not covered by `query.rs`, starting with 128-bit integers.
//!
//! Every test that needs a server gets its own database so the suite can run in parallel
//! and `cargo test` can be repeated without cleaning anything up first.
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::prelude::*;
use besu_mysql::Sqlx;
use sqlx::MySqlPool;

/// The MySQL server the suite runs against.
const SERVER: &str = "mysql://root:besu@localhost:53306";

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "wide_numbers")]
pub struct WideNumber {
    #[besu(primary_key)]
    pub id: i64,
    pub value: u128,
    pub signed: i128,
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

#[tokio::test]
async fn u128_round_trips_at_the_extremes() {
    let db = fresh("besu_u128_extremes").await;
    db.push([wide_numbers.def()]).await.unwrap();

    for (id, value) in [0, 1, u64::MAX as u128, u128::MAX].into_iter().enumerate() {
        db.insert(wide_numbers)
            .value(wide_numbers.insert(id as i64, value, 0))
            .await
            .unwrap();

        let stored: Vec<WideNumber> = db
            .select(wide_numbers)
            .filter(wide_numbers.id.eq(id as i64))
            .await
            .unwrap();
        assert_eq!(stored[0].value, value);
    }
}

#[tokio::test]
async fn i128_round_trips_at_the_extremes() {
    let db = fresh("besu_i128_extremes").await;
    db.push([wide_numbers.def()]).await.unwrap();

    for (id, signed) in [0, -1, i128::MIN, i128::MAX].into_iter().enumerate() {
        db.insert(wide_numbers)
            .value(wide_numbers.insert(id as i64, 0u128, signed))
            .await
            .unwrap();

        let stored: Vec<WideNumber> = db
            .select(wide_numbers)
            .filter(wide_numbers.id.eq(id as i64))
            .await
            .unwrap();
        assert_eq!(stored[0].signed, signed);
    }
}

#[tokio::test]
async fn u128_round_trips_through_a_transaction() {
    let db = fresh("besu_u128_transaction").await;
    db.push([wide_numbers.def()]).await.unwrap();

    db.transaction(async |tx| {
        tx.insert(wide_numbers)
            .value(wide_numbers.insert(1, u128::MAX, i128::MIN))
            .await?;
        Ok(())
    })
    .await
    .unwrap();

    let stored: Vec<WideNumber> = db.select(wide_numbers).await.unwrap();
    assert_eq!(stored[0].value, u128::MAX);
    assert_eq!(stored[0].signed, i128::MIN);
}

#[tokio::test]
async fn declared_type_is_a_128_bit_unsigned_integer() {
    let schema = besu::Database::<Sqlx>::schema([wide_numbers.def()]);
    let table = schema.table("wide_numbers").unwrap();
    assert_eq!(table.column("value").unwrap().ty, besu::TypeDef::UInt(128));
    assert_eq!(
        table.column("signed").unwrap().ty,
        besu::TypeDef::Decimal(39, 0)
    );

    let db = fresh("besu_u128_declared").await;
    let first = db.push([wide_numbers.def()]).await.unwrap();
    assert!(
        first.statements[0].contains("`value` DECIMAL(39,0)"),
        "a u128 column must be declared as DECIMAL(39,0): {:?}",
        first.statements
    );

    // `DECIMAL(39,0)` introspects back as the same width, so a second push is a no-op.
    let second = db.push([wide_numbers.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);
}
