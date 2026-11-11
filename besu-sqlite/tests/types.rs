//! Round-trip every supported column type through SQLite.
#![allow(missing_docs, clippy::too_many_arguments)]
#![allow(clippy::unwrap_used, clippy::panic)]

use besu::prelude::*;
use besu_sqlite::Sqlx;

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
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.push([things.def()]).await.unwrap();

    let id = uuid::Uuid::new_v4();
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
        unsigned_big: 10_000_000_000,
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
    assert!(table.column("maybe").unwrap().nullable);
    assert!(!table.column("text").unwrap().nullable);
}
