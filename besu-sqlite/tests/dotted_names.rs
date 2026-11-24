//! A literal dot in an unqualified table name is not a schema separator.
#![cfg(any(feature = "native", feature = "sqlx"))]
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::prelude::*;
#[cfg(feature = "native")]
use besu_sqlite::Native;
#[cfg(feature = "sqlx")]
use besu_sqlite::Sqlx;

mod integer_version {
    use besu::Table;

    #[derive(Table, Debug, Clone)]
    #[besu(table = "order.items")]
    pub struct OrderItem {
        #[besu(primary_key)]
        pub id: i64,
    }
}

mod text_version {
    use besu::Table;

    #[derive(Table, Debug, Clone)]
    #[besu(table = "order.items")]
    pub struct OrderItem {
        #[besu(primary_key)]
        pub id: String,
    }
}

async fn a_literal_dotted_name_is_created_and_rebuilt_in_main<Dr>(driver: Dr)
where
    Dr: besu::Driver<Dialect = besu_sqlite::SQLite> + besu::SchemaDriver,
    Dr::Error: core::fmt::Debug,
    String: besu::Decode<Dr>,
{
    let db = besu::Database::new(driver);
    db.execute(sql!(r#"ATTACH DATABASE ':memory:' AS "order""#))
        .await
        .unwrap();
    db.push([integer_version::order_items.def()]).await.unwrap();

    let main_tables: Vec<String> = db
        .execute(sql!(
            r#"SELECT name FROM main.sqlite_master WHERE type = 'table' AND name = 'order.items'"#
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(main_tables, ["order.items"]);
    let attached_tables: Vec<String> = db
        .execute(sql!(
            r#"SELECT name FROM "order".sqlite_master WHERE type = 'table' AND name = 'items'"#
        ))
        .all()
        .await
        .unwrap();
    assert!(attached_tables.is_empty(), "{attached_tables:?}");

    db.execute(sql!(r#"INSERT INTO main."order.items" (id) VALUES (7)"#))
        .await
        .unwrap();
    let applied = db.push([text_version::order_items.def()]).await.unwrap();
    assert!(!applied.statements.is_empty(), "{:?}", applied.changes);
    assert!(applied.changes.iter().any(|change| matches!(
        change,
        besu::schema::Change::AlterColumn { table, .. } if table == r#""order.items""#
    )));
    let stored: Vec<(String, String)> = db
        .execute(sql!(r#"SELECT typeof(id), id FROM main."order.items""#))
        .all()
        .await
        .unwrap();
    assert_eq!(stored, [("text".to_owned(), "7".to_owned())]);
    assert!(
        db.push([text_version::order_items.def()])
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
#[cfg(feature = "native")]
async fn native_a_literal_dotted_name_is_created_and_rebuilt_in_main() {
    a_literal_dotted_name_is_created_and_rebuilt_in_main(Native::in_memory().unwrap()).await;
}

#[tokio::test]
#[cfg(feature = "sqlx")]
async fn sqlx_a_literal_dotted_name_is_created_and_rebuilt_in_main() {
    a_literal_dotted_name_is_created_and_rebuilt_in_main(Sqlx::memory().await.unwrap()).await;
}
