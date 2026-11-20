//! Changes to a PostgreSQL column's single-column UNIQUE constraint.
#![cfg(feature = "sqlx")]
#![allow(clippy::unwrap_used)]

mod common;

use besu::{
    TypeDef,
    schema::{Column, Ddl},
};
use besu_postgres::PostgresDdl;
use sqlx::{Connection, PgConnection};

fn column(name: &str, unique: bool) -> Column {
    Column {
        name: name.into(),
        ty: TypeDef::Text,
        nullable: false,
        primary_key: false,
        auto_increment: false,
        unique,
        default: None,
        generated: None,
        comment: None,
        renamed_from: None,
    }
}

#[tokio::test]
async fn adding_column_unique_rejects_duplicates() {
    let mut db = PgConnection::connect(common::URL).await.unwrap();
    sqlx::query("CREATE TEMP TABLE besu_audit_add_single_unique (email TEXT NOT NULL)")
        .execute(&mut db)
        .await
        .unwrap();

    let ddl = PostgresDdl.alter_column(
        "pg_temp.besu_audit_add_single_unique",
        &column("email", false),
        &column("email", true),
    );
    for statement in ddl {
        sqlx::query(&statement).execute(&mut db).await.unwrap();
    }

    let result = sqlx::query(
        "INSERT INTO besu_audit_add_single_unique VALUES ('alice'), ('alice') ON CONFLICT DO NOTHING",
    )
    .execute(&mut db)
    .await
    .unwrap();
    assert_eq!(result.rows_affected(), 1);
}

#[tokio::test]
async fn dropping_column_unique_uses_the_catalog_constraint_name() {
    let mut db = PgConnection::connect(common::URL).await.unwrap();
    sqlx::query(
        r#"CREATE TEMP TABLE besu_audit_drop_single_unique (
            "e'mail" TEXT NOT NULL CONSTRAINT "email's uniqueness" UNIQUE,
            handle TEXT CONSTRAINT besu_audit_drop_single_unique_email_key UNIQUE
        )"#,
    )
    .execute(&mut db)
    .await
    .unwrap();

    for statement in PostgresDdl.alter_column(
        "pg_temp.besu_audit_drop_single_unique",
        &column("e'mail", true),
        &column("e'mail", false),
    ) {
        sqlx::query(&statement).execute(&mut db).await.unwrap();
    }

    let result = sqlx::query(
        "INSERT INTO besu_audit_drop_single_unique VALUES \
         ('alice', 'first'), ('alice', 'second'), ('other', 'first') ON CONFLICT DO NOTHING",
    )
    .execute(&mut db)
    .await
    .unwrap();
    assert_eq!(result.rows_affected(), 2);
}

#[tokio::test]
async fn dropping_column_unique_refuses_ambiguous_constraints() {
    let mut db = PgConnection::connect(common::URL).await.unwrap();
    sqlx::query(
        "CREATE TEMP TABLE besu_audit_ambiguous_single_unique (
            email TEXT NOT NULL,
            CONSTRAINT immediate_unique UNIQUE (email),
            CONSTRAINT deferred_unique UNIQUE (email) DEFERRABLE INITIALLY DEFERRED
        )",
    )
    .execute(&mut db)
    .await
    .unwrap();

    let statements = PostgresDdl.alter_column(
        "pg_temp.besu_audit_ambiguous_single_unique",
        &column("email", true),
        &column("email", false),
    );
    assert_eq!(statements.len(), 1);
    let error = sqlx::query(&statements[0])
        .execute(&mut db)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("0A000")
    );
    let constraints: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_constraint
         WHERE conrelid = 'pg_temp.besu_audit_ambiguous_single_unique'::regclass AND contype = 'u'",
    )
    .fetch_one(&mut db)
    .await
    .unwrap();
    assert_eq!(constraints, 2);
}
