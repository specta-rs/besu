//! MySQL's own type system: the inline `SET` and the fixed-width `BIT`.
#![cfg(feature = "sqlx")]
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::prelude::*;
use besu::{BitString, EnumLabels, TypeDef, Value};
use besu_mysql::Sqlx;
use sqlx::MySqlPool;

/// The MySQL server the suite runs against.
const SERVER: &str = "mysql://root:besu@localhost:53306";

/// A `SET('read','write','admin')` column type.
///
/// MySQL has no named `SET` type, so the labels belong to the column. A `SqlType` written by
/// hand is the declaration: its `DEF` carries the label list and its Rust type is the
/// comma-separated value MySQL itself sends.
struct Permissions;

impl SqlType for Permissions {
    const DEF: TypeDef = TypeDef::Set(EnumLabels::from_static(&["read", "write", "admin"]));
    type Rust = String;
}

impl IntoSqlValue<Permissions> for String {
    fn into_sql_value(self) -> Value {
        Value::Text(self)
    }
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "accounts")]
pub struct Account {
    #[besu(primary_key)]
    pub id: i64,
    #[besu(sql_type = "Permissions")]
    pub permissions: String,
    #[besu(sql_type = "besu::Bit<8>")]
    pub flags: BitString,
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
async fn a_set_and_a_bit_round_trip() {
    let db = fresh("besu_set_and_bit").await;
    db.push([accounts.def()]).await.unwrap();

    let second = db.push([accounts.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);

    let original = Account {
        id: 1,
        permissions: "read,admin".into(),
        // The top bit is set, so the value is its own leading-zero-free form and the bit
        // string survives MySQL's integer encoding exactly.
        flags: BitString::new("10110011").unwrap(),
    };
    db.insert(accounts)
        .value(accounts.insert(
            original.id,
            original.permissions.clone(),
            original.flags.clone(),
        ))
        .await
        .unwrap();

    let stored: Vec<Account> = db.select(accounts).await.unwrap();
    assert_eq!(stored, [original]);
}

#[tokio::test]
async fn every_bit_of_a_byte_round_trips() {
    let db = fresh("besu_bit_values").await;
    db.push([accounts.def()]).await.unwrap();

    // A `BIT(8)` value arrives as one byte. A value with a leading zero bit is read back as
    // its significant bits, because MySQL keeps the value as an integer and the width is not
    // on the wire; every other value round-trips exactly.
    let values = ["10000000", "11111111", "00000000"];
    for (index, bits) in values.into_iter().enumerate() {
        let id = index as i64;
        db.insert(accounts)
            .value(accounts.insert(id, "read".to_owned(), BitString::new(bits).unwrap()))
            .await
            .unwrap();

        let stored: Vec<Account> = db
            .select(accounts)
            .filter(accounts.id.eq(id))
            .await
            .unwrap();
        let expected = if bits == "00000000" { "0" } else { bits };
        assert_eq!(stored[0].flags.as_str(), expected);
    }
}

#[tokio::test]
async fn the_set_and_bit_types_are_in_the_schema_and_introspect_back() {
    let db = fresh("besu_set_and_bit_schema").await;
    db.push([accounts.def()]).await.unwrap();

    let declared = besu::Database::<Sqlx>::schema([accounts.def()]);
    let table = declared.table("accounts").unwrap();
    assert_eq!(
        table.column("permissions").unwrap().ty,
        TypeDef::Set(EnumLabels::from_static(&["read", "write", "admin"]))
    );
    assert_eq!(table.column("flags").unwrap().ty, TypeDef::Bit(8));

    let introspected = db.introspect().await.unwrap();
    let table = introspected.table("accounts").unwrap();
    assert_eq!(
        table.column("permissions").unwrap().ty,
        TypeDef::Set(EnumLabels::from_static(&["read", "write", "admin"]))
    );
    assert_eq!(table.column("flags").unwrap().ty, TypeDef::Bit(8));

    // The DDL is `SET(...)` and `BIT(8)`, and the column type converges exactly.
    let pushed = db.push_schema(&introspected).await.unwrap();
    assert!(pushed.is_empty(), "got {:?}", pushed.changes);
}
