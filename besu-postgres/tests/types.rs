#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_arguments
)]

//! Round-trip every supported column type through PostgreSQL.

mod common;

use besu::prelude::*;
use besu_postgres::Sqlx;

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "things")]
pub struct Thing {
    #[besu(primary_key)]
    pub id: uuid::Uuid,
    pub flag: bool,
    pub tiny: i8,
    pub small: i16,
    pub normal: i32,
    pub big: i64,
    pub unsigned_tiny: u8,
    pub unsigned_small: u16,
    pub unsigned_normal: u32,
    pub unsigned_big: u64,
    pub ratio: f32,
    pub precise: f64,
    pub text: String,
    pub blob: Vec<u8>,
    pub when_date: chrono::NaiveDate,
    pub when_time: chrono::NaiveTime,
    pub when_timestamp: chrono::NaiveDateTime,
    pub when_timestamp_tz: chrono::DateTime<chrono::Utc>,
    pub document: serde_json::Value,
    pub maybe: Option<String>,
}

#[tokio::test]
async fn every_type_round_trips() {
    let db = common::database().await;
    common::reset(&db).await;
    db.push([things.def()]).await.unwrap();

    // Every declared type maps onto a native type that introspection reports back, so a
    // second push changes nothing.
    let second = db.push([things.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);

    let id = uuid::Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
    let original = Thing {
        id,
        flag: true,
        tiny: -8,
        small: -1_000,
        normal: -100_000,
        big: -10_000_000_000,
        unsigned_tiny: 200,
        unsigned_small: 60_000,
        unsigned_normal: 4_000_000_000,
        // Above `i64::MAX`, which is the only way a `u64` is not just an `i64`.
        unsigned_big: 10_000_000_000_000_000_000,
        ratio: 1.5,
        precise: 2.25,
        text: "hello".into(),
        blob: vec![1, 2, 3, 255],
        when_date: chrono::NaiveDate::from_ymd_opt(2024, 5, 17).unwrap(),
        when_time: chrono::NaiveTime::from_hms_opt(13, 45, 6).unwrap(),
        when_timestamp: chrono::NaiveDateTime::new(
            chrono::NaiveDate::from_ymd_opt(2024, 5, 17).unwrap(),
            chrono::NaiveTime::from_hms_opt(13, 45, 6).unwrap(),
        ),
        when_timestamp_tz: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        document: serde_json::json!({ "a": 1, "b": [true, null] }),
        maybe: None,
    };

    db.insert(things)
        .value(things.insert(
            id,
            true,
            original.tiny,
            original.small,
            original.normal,
            original.big,
            original.unsigned_tiny,
            original.unsigned_small,
            original.unsigned_normal,
            original.unsigned_big,
            original.ratio,
            original.precise,
            original.text.clone(),
            original.blob.clone(),
            original.when_date,
            original.when_time,
            original.when_timestamp,
            original.when_timestamp_tz,
            original.document.clone(),
        ))
        .await
        .unwrap();

    let stored: Vec<Thing> = db.select(things).await.unwrap();
    assert_eq!(stored.len(), 1);
    let stored = &stored[0];
    assert_eq!(stored.id, original.id);
    assert_eq!(stored.flag, original.flag);
    assert_eq!(stored.tiny, original.tiny);
    assert_eq!(stored.small, original.small);
    assert_eq!(stored.normal, original.normal);
    assert_eq!(stored.big, original.big);
    assert_eq!(stored.unsigned_tiny, original.unsigned_tiny);
    assert_eq!(stored.unsigned_small, original.unsigned_small);
    assert_eq!(stored.unsigned_normal, original.unsigned_normal);
    assert_eq!(stored.unsigned_big, original.unsigned_big);
    assert_eq!(stored.ratio, original.ratio);
    assert_eq!(stored.precise, original.precise);
    assert_eq!(stored.text, original.text);
    assert_eq!(stored.blob, original.blob);
    assert_eq!(stored.when_date, original.when_date);
    assert_eq!(stored.when_time, original.when_time);
    assert_eq!(stored.when_timestamp, original.when_timestamp);
    assert_eq!(stored.when_timestamp_tz, original.when_timestamp_tz);
    assert_eq!(stored.document, original.document);
    assert_eq!(stored.maybe, original.maybe);
}

/// A table whose only job is to hold a `u64` at its extremes.
///
/// The values round-trip through a real `NUMERIC(20,0)` column, which is the type the derive
/// declares for `u64`, rather than through a cast in a `SELECT`.
#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "numbers")]
pub struct Number {
    #[besu(primary_key)]
    pub id: i64,
    pub value: u64,
}

#[tokio::test]
async fn unsigned_big_round_trips_at_the_extremes() {
    let db = common::database().await;
    common::reset(&db).await;
    db.push([numbers.def()]).await.unwrap();

    for (id, value) in [0, 1, i64::MAX as u64 + 1, u64::MAX]
        .into_iter()
        .enumerate()
    {
        db.insert(numbers)
            .value(numbers.insert(id as i64, value))
            .await
            .unwrap();

        let stored: Vec<Number> = db
            .select(numbers)
            .filter(numbers.id.eq(id as i64))
            .await
            .unwrap();
        assert_eq!(stored[0].value, value);
    }
}

#[tokio::test]
async fn declared_types_are_reflected_in_the_schema() {
    let schema = besu::Database::<Sqlx>::schema([things.def()]);
    let table = schema.table("things").unwrap();

    assert_eq!(table.column("id").unwrap().ty, besu::TypeDef::Uuid);
    assert!(table.column("id").unwrap().primary_key);
    assert!(!table.column("id").unwrap().auto_increment);
    assert_eq!(table.column("blob").unwrap().ty, besu::TypeDef::Bytes);
    assert_eq!(table.column("document").unwrap().ty, besu::TypeDef::Json);
    assert_eq!(
        table.column("when_timestamp_tz").unwrap().ty,
        besu::TypeDef::TimestampTz
    );
    assert_eq!(
        table.column("unsigned_big").unwrap().ty,
        besu::TypeDef::UInt(64)
    );
    assert!(table.column("maybe").unwrap().nullable);
    assert!(!table.column("text").unwrap().nullable);
}

/// A table holding 128-bit integers, which PostgreSQL spells `NUMERIC(39,0)`.
#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "wide_numbers")]
pub struct WideNumber {
    #[besu(primary_key)]
    pub id: i64,
    pub value: u128,
    pub signed: i128,
}

#[tokio::test]
async fn u128_round_trips_at_the_extremes() {
    let db = common::database().await;
    common::reset(&db).await;
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
    let db = common::database().await;
    common::reset(&db).await;
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
    let db = common::database().await;
    common::reset(&db).await;
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

    let db = common::database().await;
    common::reset(&db).await;
    let first = db.push([wide_numbers.def()]).await.unwrap();
    assert!(
        first.statements[0].contains("\"value\" NUMERIC(39,0)"),
        "a u128 column must be declared as NUMERIC(39,0): {:?}",
        first.statements
    );

    // `NUMERIC(39,0)` introspects back as the same width, so a second push is a no-op.
    let second = db.push([wide_numbers.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);
}
