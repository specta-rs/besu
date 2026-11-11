//! 128-bit integers through SQLite, where they are stored as decimal text.
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::prelude::*;
use besu_sqlite::Sqlx;

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
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
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
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
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
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
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

    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    let first = db.push([wide_numbers.def()]).await.unwrap();
    // SQLite has no 128-bit integer, so the column is `TEXT` holding the decimal digits.
    assert!(
        first.statements[0].contains("\"value\" TEXT"),
        "a u128 column must be declared as text: {:?}",
        first.statements
    );

    // The declared type folds onto `TEXT` next to the database's own report, so a second
    // push changes nothing.
    let second = db.push([wide_numbers.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);
}
