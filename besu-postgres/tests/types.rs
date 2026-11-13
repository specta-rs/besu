#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_arguments
)]

//! Round-trip every supported column type through PostgreSQL.
#![cfg(feature = "sqlx")]

mod common;

use besu::prelude::*;
use besu::{Decimal, Driver, Sql};
use besu_postgres::{Postgres, Sqlx};

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

#[tokio::test]
async fn invalid_json_is_rejected_before_it_reaches_the_server() {
    let db = common::database().await;
    common::reset(&db).await;

    let mut statement = Sql::<Postgres>::raw("SELECT $1::jsonb");
    statement.push_arg(Value::Json(br#"{"a":"#.to_vec()));

    // The driver validates the document in place rather than parsing it into a tree, and a
    // malformed one still has to fail before the statement is sent.
    let error = db.driver().query(&statement).await.unwrap_err();
    assert!(
        error.to_string().contains("cannot encode value `json`"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn round_numbers_round_trip() {
    let db = common::database().await;
    common::reset(&db).await;
    db.push([numbers.def(), wide_numbers.def()]).await.unwrap();

    // A power of ten leaves a gap between a `NUMERIC`'s weight and its stored digits — 10^8
    // is one digit with weight 2 — which a hand-written decoder gets wrong if it assumes the
    // digits are contiguous.
    for (id, value) in [1_u64, 10, 10_u64.pow(4), 10_u64.pow(8), 10_u64.pow(19)]
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

    for (id, value) in [
        10_u128.pow(4),
        10_u128.pow(8),
        10_u128.pow(20),
        10_u128.pow(38),
    ]
    .into_iter()
    .enumerate()
    {
        let signed = -(value as i128);
        db.insert(wide_numbers)
            .value(wide_numbers.insert(id as i64, value, signed))
            .await
            .unwrap();

        let stored: Vec<WideNumber> = db
            .select(wide_numbers)
            .filter(wide_numbers.id.eq(id as i64))
            .await
            .unwrap();
        assert_eq!(stored[0].value, value);
        assert_eq!(stored[0].signed, signed);
    }
}

// ---------------------------------------------------------------------------------------
// Exact decimals
// ---------------------------------------------------------------------------------------

/// A decimal column whose Rust type is the exact text.
#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "ledger")]
pub struct Ledger {
    #[besu(primary_key)]
    pub id: i64,
    /// `NUMERIC(12,2)`, which `Decimal` maps onto.
    #[besu(sql_type = "besu::Decimal<12,2>")]
    pub amount: String,
}

/// A `Decimal` is written and read as its exact text, at a scale that a float could not keep.
#[tokio::test]
async fn a_decimal_round_trips_exactly() {
    let db = common::database().await;
    common::reset(&db).await;
    db.push([ledger.def()]).await.unwrap();

    // The declared type comes back as the same width, so a second push changes nothing.
    let second = db.push([ledger.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);

    for (id, amount) in [
        (1_i64, "0.00"),
        (2, "1.23"),
        (3, "-1234567890.99"),
        (4, "9999999999.99"),
    ] {
        db.insert(ledger)
            .value(ledger.insert(id, amount.to_owned()))
            .await
            .unwrap();
    }

    let stored: Vec<Ledger> = db.select(ledger).order_by(ledger.id.asc()).await.unwrap();
    assert_eq!(
        stored
            .iter()
            .map(|row| row.amount.as_str())
            .collect::<Vec<_>>(),
        ["0.00", "1.23", "-1234567890.99", "9999999999.99"]
    );

    // A decimal is bound as `NUMERIC`, not as text, so an equality comparison resolves.
    let found: Vec<Ledger> = db
        .select(ledger)
        .filter(ledger.amount.eq("1.23"))
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, 2);
}

/// `sum_as` and `avg_as` add up a decimal without ever passing through a double.
#[tokio::test]
async fn a_decimal_sums_exactly() {
    use besu::expr::{AvgAs, SumAs};

    let db = common::database().await;
    common::reset(&db).await;
    db.push([ledger.def()]).await.unwrap();

    for (id, amount) in [(1_i64, "0.10"), (2, "0.20")] {
        db.insert(ledger)
            .value(ledger.insert(id, amount.to_owned()))
            .await
            .unwrap();
    }

    // `0.1 + 0.2` is the classic float failure; an exact sum is `0.30`.
    let sums: Vec<String> = db
        .select(ledger)
        .pick(ledger.amount.sum_as::<Decimal<20, 2>>())
        .all()
        .await
        .unwrap();
    assert_eq!(sums[0], "0.30");

    let averages: Vec<String> = db
        .select(ledger)
        .pick(ledger.amount.avg_as::<Decimal<20, 4>>())
        .all()
        .await
        .unwrap();
    assert!(averages[0].starts_with("0.15"), "got {:?}", averages[0]);
}

/// `NUMERIC(20,0)` is no longer guessed to be a `u64`: a twenty-digit numeric can be larger
/// than `u64::MAX`, so the column is reported as the decimal it is.
#[tokio::test]
async fn a_numeric_20_is_a_decimal_not_a_guess() {
    let db = common::database().await;
    common::reset(&db).await;
    db.execute(sql!("CREATE TABLE wide (v NUMERIC(20,0) NOT NULL)"))
        .await
        .unwrap();

    let schema = db.introspect().await.unwrap();
    assert_eq!(
        schema.table("wide").unwrap().column("v").unwrap().ty,
        TypeDef::Decimal(20, 0)
    );
    // The declared `u64` folds onto the same decimal, so convergence survives the change.
    let mut declared = schema.clone();
    declared.tables[0].columns[0].ty = TypeDef::UInt(64);
    assert!(db.push_schema(&declared).await.unwrap().is_empty());
    // And the snapshot replays to the schema that is already there.
    assert!(db.push_schema(&schema).await.unwrap().is_empty());
}

// ---------------------------------------------------------------------------------------
// Arrays
// ---------------------------------------------------------------------------------------

/// A table with arrays of text and of integers, which the derive infers from `Vec<T>`.
#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "articles")]
pub struct Article {
    #[besu(primary_key)]
    pub id: i64,
    pub tags: Vec<String>,
    pub scores: Vec<i32>,
}

#[tokio::test]
async fn an_array_is_declared_and_converges() {
    let db = common::database().await;
    common::reset(&db).await;
    let first = db.push([articles.def()]).await.unwrap();
    let statement = &first.statements[0];
    assert!(
        statement.contains(r#""tags" TEXT[] NOT NULL"#),
        "{statement}"
    );
    assert!(
        statement.contains(r#""scores" INTEGER[] NOT NULL"#),
        "{statement}"
    );

    // `int[]` used to introspect as `Custom("_int4")`, so this second push was a rename of
    // the column's type. It is now the same type twice.
    let second = db.push([articles.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);
}

#[tokio::test]
async fn an_array_round_trips_and_queries() {
    let db = common::database().await;
    common::reset(&db).await;
    db.push([articles.def()]).await.unwrap();

    db.insert(articles)
        .value(articles.insert(1, vec!["rust".into(), "sql".into()], vec![10, 20]))
        .await
        .unwrap();
    db.insert(articles)
        .value(articles.insert(2, Vec::new(), vec![30]))
        .await
        .unwrap();

    let stored: Vec<Article> = db
        .select(articles)
        .order_by(articles.id.asc())
        .await
        .unwrap();
    assert_eq!(stored[0].tags, ["rust", "sql"]);
    assert_eq!(stored[0].scores, [10, 20]);
    assert!(stored[1].tags.is_empty());
    assert_eq!(stored[1].scores, [30]);
}

#[tokio::test]
async fn array_operators_are_rendered_for_postgres() {
    use besu::expr::{
        array_append, array_contains, array_length, array_overlaps, array_position, cardinality,
        in_list,
    };

    let db = common::database().await;
    common::reset(&db).await;
    db.push([articles.def()]).await.unwrap();

    db.insert(articles)
        .value(articles.insert(1, vec!["rust".into(), "sql".into()], vec![10, 20]))
        .await
        .unwrap();
    db.insert(articles)
        .value(articles.insert(2, vec!["go".into()], vec![30]))
        .await
        .unwrap();

    let containing: Vec<Article> = db
        .select(articles)
        .filter(array_contains(articles.tags, vec!["rust".to_owned()]))
        .await
        .unwrap();
    assert_eq!(containing.len(), 1);

    let overlapping: Vec<Article> = db
        .select(articles)
        .filter(array_overlaps(articles.tags, vec!["go".to_owned()]))
        .await
        .unwrap();
    assert_eq!(overlapping.len(), 1);
    assert_eq!(overlapping[0].id, 2);

    // `in_list` binds one array parameter rather than one per value.
    let listed: Vec<Article> = db
        .select(articles)
        .filter(in_list(articles.id, [1_i64, 2]))
        .order_by(articles.id.asc())
        .await
        .unwrap();
    assert_eq!(listed.len(), 2);

    let lengths: Vec<(i64, i64)> = db
        .select(articles)
        .pick((array_length(articles.tags, 1), cardinality(articles.scores)))
        .filter(articles.id.eq(1_i64))
        .all()
        .await
        .unwrap();
    assert_eq!(lengths[0], (2, 2));

    let position: Vec<i64> = db
        .select(articles)
        .pick(array_position(articles.scores, 20))
        .filter(articles.id.eq(1_i64))
        .all()
        .await
        .unwrap();
    assert_eq!(position[0], 2);

    let appended: Vec<Vec<String>> = db
        .select(articles)
        .pick(array_append(articles.tags, "db"))
        .filter(articles.id.eq(2_i64))
        .all()
        .await
        .unwrap();
    assert_eq!(appended[0], ["go", "db"]);
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "arrays_of_numbers")]
pub struct Numbers {
    #[besu(primary_key)]
    pub id: i64,
    pub amounts: Vec<u128>,
}

/// An element whose Rust type needs its own conversion (`u128` → `UInt128`, `String` in a
/// decimal array → `Decimal`) must still encode: the element type travels with the values.
#[tokio::test]
async fn array_elements_use_the_declared_element_type() {
    let db = common::database().await;
    common::reset(&db).await;
    db.push([arrays_of_numbers.def()]).await.unwrap();

    db.insert(arrays_of_numbers)
        .value(arrays_of_numbers.insert(1_i64, vec![0, u128::MAX]))
        .await
        .unwrap();

    let stored: Vec<Numbers> = db.select(arrays_of_numbers).all().await.unwrap();
    assert_eq!(stored[0].amounts, [0, u128::MAX]);
}
