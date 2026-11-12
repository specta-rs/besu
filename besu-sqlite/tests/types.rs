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

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "prices")]
pub struct Price {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    // `sql_type` names a `SqlType` directly, which is how a sized or exact type is declared.
    #[besu(sql_type = "besu::Decimal<10, 2>")]
    pub amount: String,
    #[besu(sql_type = "besu::VarChar<64>")]
    pub label: String,
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "moments")]
pub struct Moment {
    #[besu(primary_key)]
    pub id: i64,
    pub day: chrono::NaiveDate,
    pub clock: chrono::NaiveTime,
    pub stamp: chrono::NaiveDateTime,
}

#[tokio::test]
async fn temporal_values_are_stored_as_sqlx_text() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.push([moments.def()]).await.unwrap();

    db.insert(moments)
        .value(moments.insert(
            1,
            chrono::NaiveDate::from_ymd_opt(2024, 5, 17).unwrap(),
            chrono::NaiveTime::from_hms_opt(13, 45, 6).unwrap(),
            chrono::NaiveDateTime::new(
                chrono::NaiveDate::from_ymd_opt(2024, 5, 17).unwrap(),
                chrono::NaiveTime::from_hms_nano_opt(13, 45, 6, 123_456_789).unwrap(),
            ),
        ))
        .await
        .unwrap();

    // The driver formats these itself instead of handing them to sqlx's encoder, so the text
    // it writes has to be exactly what sqlx decodes: `%F`, `%T%.f` and `%F %T%.f`.
    let stored: Vec<(String, String, String)> = db
        .execute(sql!(
            "SELECT \"day\", \"clock\", \"stamp\" FROM \"moments\""
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(stored[0].0, "2024-05-17");
    assert_eq!(stored[0].1, "13:45:06");
    assert_eq!(stored[0].2, "2024-05-17 13:45:06.123456789");

    let round_tripped: Vec<Moment> = db.select(moments).await.unwrap();
    assert_eq!(round_tripped.len(), 1);
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "events")]
pub struct Event {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    #[besu(index)]
    pub kind: String,
    #[besu(unique)]
    pub slug: String,
    pub payload: serde_json::Value,
}

#[tokio::test]
async fn declared_indexes_are_created_and_converge() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());

    let first = db.push([events.def()]).await.unwrap();
    assert!(
        first
            .statements
            .iter()
            .any(|s| s.contains("CREATE INDEX \"events_kind_idx\"")),
        "a declared index must be created: {:?}",
        first.statements
    );

    // If indexes were not created with the table, the second push would report AddIndex and
    // `push` would never converge.
    let second = db.push([events.def()]).await.unwrap();
    assert!(
        second.is_empty(),
        "second push should be a no-op, got {:?}",
        second.changes
    );

    let introspected = db.introspect().await.unwrap();
    let table = introspected.table("events").unwrap();
    assert!(
        table.indexes.iter().any(|i| i.name == "events_kind_idx"),
        "the index must be visible to introspection: {:?}",
        table.indexes
    );
    assert!(
        table.column("slug").unwrap().unique,
        "a unique column must round-trip as unique"
    );
}

#[tokio::test]
async fn explicit_sql_types_are_declared_and_converge() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    let first = db.push([prices.def()]).await.unwrap();
    // SQLite has no decimal type, and `NUMERIC` affinity would corrupt an exact value, so a
    // decimal column is text there. The declared type is still a decimal.
    assert!(
        first.statements[0].contains("\"amount\" TEXT"),
        "a decimal column must be stored as text on SQLite: {:?}",
        first.statements
    );

    let schema = besu::Database::<Sqlx>::schema([prices.def()]);
    let table = schema.table("prices").unwrap();
    assert_eq!(
        table.column("amount").unwrap().ty,
        besu::TypeDef::Decimal(10, 2)
    );
    assert_eq!(
        table.column("label").unwrap().ty,
        besu::TypeDef::VarChar(64)
    );

    db.insert(prices)
        .value(prices.insert("12.34", "widget"))
        .await
        .unwrap();
    let stored: Vec<Price> = db.select(prices).await.unwrap();
    assert_eq!(stored[0].amount, "12.34");

    // SQLite stores both as NUMERIC/TEXT, so a second push is still a no-op.
    let second = db.push([prices.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);
}

// ---------------------------------------------------------------------------------------
// Exact decimals
// ---------------------------------------------------------------------------------------

/// SQLite has no decimal type, so a `Decimal` is stored as its exact text.
#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "money")]
pub struct Money {
    #[besu(primary_key)]
    pub id: i64,
    #[besu(sql_type = "besu::Decimal<10,2>")]
    pub amount: String,
}

#[tokio::test]
async fn a_decimal_is_stored_as_exact_text() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.push([money.def()]).await.unwrap();
    let second = db.push([money.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);

    for (id, amount) in [(1_i64, "0.10"), (2, "0.20")] {
        db.insert(money)
            .value(money.insert(id, amount.to_owned()))
            .await
            .unwrap();
    }

    let stored: Vec<Money> = db.select(money).order_by(money.id.asc()).await.unwrap();
    assert_eq!(
        stored
            .iter()
            .map(|row| row.amount.as_str())
            .collect::<Vec<_>>(),
        ["0.10", "0.20"]
    );
}

/// SQLite has no array type, so binding one fails before the statement is sent.
#[tokio::test]
async fn an_array_is_rejected_before_it_reaches_the_server() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    let statement = besu::Fragment::with_args(
        "SELECT ?",
        vec![Value::Array {
            element: TypeDef::Text,
            values: vec![Value::Text("a".into())],
        }],
    );
    let error = db.execute(statement).await.unwrap_err();
    assert!(
        error.to_string().contains("no array type"),
        "unexpected error: {error}"
    );
}
