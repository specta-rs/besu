//! Table prefixes, for deployments that share one physical database between tenants.
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::prelude::*;
use besu_sqlite::{SQLite, Sqlx};

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "users")]
pub struct User {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub name: String,
}

#[tokio::test]
async fn every_rendered_table_name_carries_the_prefix() {
    let db = besu::Database::prefixed(Sqlx::memory().await.unwrap(), "tenant_1_");

    let sql = db
        .select(users)
        .filter(users.name.eq("Alice"))
        .to_sql()
        .unwrap();
    assert!(
        sql.sql().contains(r#""tenant_1_users"."name""#),
        "a qualified column must be prefixed: {}",
        sql.sql()
    );
    assert!(
        sql.sql().contains(r#"FROM "tenant_1_users""#),
        "the FROM clause must be prefixed: {}",
        sql.sql()
    );

    let insert = db
        .insert(users)
        .value(users.insert("Alice"))
        .to_sql()
        .unwrap();
    assert!(insert.sql().contains(r#"INSERT INTO "tenant_1_users""#));

    // An unprefixed database is unchanged.
    let dry = besu::dry::<SQLite>();
    let sql = dry.select(users).to_sql().unwrap();
    assert!(sql.sql().contains(r#"FROM "users""#), "{}", sql.sql());

    // A derived table's body is a table in the database and takes the prefix; its alias is
    // a name in the statement and must not.
    let prefixed = besu::Database::prefixed(Sqlx::memory().await.unwrap(), "tenant_1_");
    let recent = derived(prefixed.select(users).pick(users.id), "recent");
    let id = recent.column(users.id);
    let sql = prefixed.select(&recent).pick(id).to_sql().unwrap();
    assert!(
        sql.sql()
            .contains(r#"FROM (SELECT "tenant_1_users"."id" FROM "tenant_1_users") AS "recent""#),
        "the derived body must be prefixed and the alias must not: {}",
        sql.sql()
    );
    assert!(!sql.sql().contains("tenant_1_recent"), "{}", sql.sql());
}

#[tokio::test]
async fn two_tenants_share_one_database() {
    let driver = Sqlx::memory().await.unwrap();
    let acme = besu::Database::prefixed(driver.clone(), "acme_");
    let globex = besu::Database::prefixed(driver, "globex_");

    acme.push([users.def()]).await.unwrap();
    globex.push([users.def()]).await.unwrap();

    acme.insert(users)
        .value(users.insert("Acme Alice"))
        .await
        .unwrap();
    globex
        .insert(users)
        .value(users.insert("Globex Bob"))
        .await
        .unwrap();

    // Each tenant sees only its own rows, and its own `push` is a no-op.
    let a: Vec<User> = acme.select(users).await.unwrap();
    let g: Vec<User> = globex.select(users).await.unwrap();
    assert_eq!(a.len(), 1);
    assert_eq!(g.len(), 1);
    assert_eq!(a[0].name, "Acme Alice");
    assert_eq!(g[0].name, "Globex Bob");

    assert!(acme.push([users.def()]).await.unwrap().is_empty());
    assert!(globex.push([users.def()]).await.unwrap().is_empty());
}

#[tokio::test]
async fn the_schema_is_prefixed_and_unprefixed_consistently() {
    let db = besu::Database::prefixed(Sqlx::memory().await.unwrap(), "t9_");
    db.push([users.def()]).await.unwrap();

    // Introspection reports physical names; `push` strips the prefix before diffing, so the
    // second push converges rather than trying to create a second table.
    assert!(db.push([users.def()]).await.unwrap().is_empty());

    let physical = db.introspect().await.unwrap();
    assert!(physical.table("t9_users").is_some());
    assert!(physical.table("users").is_none());
}
