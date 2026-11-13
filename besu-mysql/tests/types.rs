//! Round-trip the types that are not covered by `query.rs`, starting with 128-bit integers.
//!
//! Every test that needs a server gets its own database so the suite can run in parallel
//! and `cargo test` can be repeated without cleaning anything up first.
#![cfg(feature = "sqlx")]
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::Decimal;
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

/// A `DECIMAL` with a scale still goes through `BigDecimal`, which truncates it, even though
/// a `DECIMAL(39,0)` is decoded from its text directly.
#[tokio::test]
async fn a_scaled_decimal_truncates_into_a_128_bit_integer() {
    let db = fresh("besu_u128_scaled_decimal").await;
    db.execute(sql!(
        "CREATE TABLE `scaled` (`value` DECIMAL(10,2) NOT NULL)"
    ))
    .await
    .unwrap();

    // Bound as text so the stored value is exactly 1.90.
    db.execute(Fragment::with_args(
        "INSERT INTO `scaled` VALUES (?)",
        vec![Value::Text("1.90".to_owned())],
    ))
    .await
    .unwrap();

    let rows: Vec<(u128,)> = db
        .execute(sql!("SELECT `value` FROM `scaled`"))
        .all()
        .await
        .unwrap();
    assert_eq!(rows[0].0, 1);
}

/// An unsigned column decodes as `i64` and as `u64`, and only the `u64` covers the top half
/// of the range.
#[tokio::test]
async fn an_unsigned_column_decodes_through_both_integer_paths() {
    let db = fresh("besu_unsigned_integer").await;
    db.execute(sql!(
        "CREATE TABLE `counter` (`value` BIGINT UNSIGNED NOT NULL)"
    ))
    .await
    .unwrap();
    db.execute(Fragment::with_args(
        "INSERT INTO `counter` VALUES (?), (?)",
        vec![Value::UInt(42), Value::UInt(u64::MAX)],
    ))
    .await
    .unwrap();

    let signed: Vec<(i64,)> = db
        .execute(sql!("SELECT `value` FROM `counter` WHERE `value` = 42"))
        .all()
        .await
        .unwrap();
    assert_eq!(signed[0].0, 42);

    let unsigned: Vec<(u64,)> = db
        .execute(sql!("SELECT `value` FROM `counter` ORDER BY `value`"))
        .all()
        .await
        .unwrap();
    assert_eq!(unsigned, [(42,), (u64::MAX,)]);

    // `u64::MAX` does not fit in `i64`, and that is an error rather than a truncation.
    let too_wide = db
        .execute(sql!(
            "SELECT `value` FROM `counter` WHERE `value` = 18446744073709551615"
        ))
        .all::<(i64,)>()
        .await;
    assert!(too_wide.is_err(), "got {too_wide:?}");
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "stamped")]
pub struct Stamped {
    #[besu(primary_key)]
    pub id: i64,
    /// `DateTime<Utc>` maps to `TimestampTz`, which MySQL stores as `DATETIME(6)` and
    /// reports back as `Timestamp`. The second `push` is what proves normalisation folds
    /// the two; before it did, this column reported a type change on every push.
    pub at: chrono::DateTime<chrono::Utc>,
    pub naive: chrono::NaiveDateTime,
}

#[tokio::test]
async fn a_timestamp_with_a_zone_converges() {
    let db = fresh("besu_test_stamped").await;
    db.push([stamped.def()]).await.unwrap();

    let second = db.push([stamped.def()]).await.unwrap();
    assert!(
        second.is_empty(),
        "push must be a no-op, got: {:?}",
        second.changes
    );
}

/// A decimal column whose Rust type is the exact text.
#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "ledger")]
pub struct Ledger {
    #[besu(primary_key)]
    pub id: i64,
    #[besu(sql_type = "besu::Decimal<10,2>")]
    pub amount: String,
}

/// MySQL coerces a string parameter into the `DECIMAL` column, and the value comes back with
/// the column's own scale.
#[tokio::test]
async fn a_decimal_round_trips_exactly() {
    use besu::expr::SumAs;

    let db = fresh("besu_decimal_round_trip").await;
    db.push([ledger.def()]).await.unwrap();

    let second = db.push([ledger.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);

    for (id, amount) in [(1_i64, "0.00"), (2, "1.23"), (3, "-98765432.10")] {
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
        ["0.00", "1.23", "-98765432.10"]
    );

    let found: Vec<Ledger> = db
        .select(ledger)
        .filter(ledger.amount.eq("1.23"))
        .await
        .unwrap();
    assert_eq!(found.len(), 1);

    let sum: Vec<String> = db
        .select(ledger)
        .pick(ledger.amount.sum_as::<Decimal<20, 2>>())
        .all()
        .await
        .unwrap();
    assert_eq!(sum[0], "-98765430.87");
}
