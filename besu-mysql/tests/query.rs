//! The MySQL driver's behaviour, mirroring `besu-sqlite/tests/query.rs`.
//!
//! Every test that needs a server gets its own database so the suite can run in parallel
//! and `cargo test` can be repeated without cleaning anything up first.
#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_arguments
)]

use besu::prelude::*;
use besu::schema::Ddl;
use besu_mysql::Sqlx;
use sqlx::MySqlPool;

/// The MySQL server the suite runs against.
const SERVER: &str = "mysql://root:besu@localhost:53306";

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "users")]
pub struct User {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    #[besu(unique)]
    pub email: String,
    pub name: String,
    pub age: i32,
    pub bio: Option<String>,
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "posts")]
pub struct Post {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub title: String,
    #[besu(references = "users.id", on_delete = "cascade")]
    pub author_id: i64,
}

/// A table with a declared secondary index, used to check index introspection.
#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "tags")]
pub struct Tag {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    #[besu(index)]
    pub name: String,
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

/// One statement with one parameter.
///
/// `sql!` asserts its placeholder count in a `const` block, which means it only accepts
/// compile-time values, so a runtime parameter goes through `Fragment` instead.
fn one(sql: &'static str, value: impl Into<Value>) -> Fragment {
    Fragment::with_args(sql, vec![value.into()])
}

/// An empty database with the reference schema already pushed.
async fn setup(name: &str) -> besu::Database<Sqlx> {
    let db = fresh(name).await;
    db.push([users.def(), posts.def()]).await.unwrap();
    db
}

#[tokio::test]
async fn insert_and_select() {
    let db = setup("besu_test_insert_and_select").await;

    // MySQL has no `RETURNING`, so the row is selected back afterwards.
    db.insert(users)
        .value(users.insert("oscar@example.com", "Oscar", 21))
        .await
        .unwrap();

    let all: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].name, "Oscar");
    assert_eq!(all[0].id, 1);
    assert_eq!(all[0].bio, None);

    // A `NULL` and a value both round-trip through the nullable column.
    db.insert(users)
        .value(
            users
                .insert("bio@example.com", "Bio", 30)
                .bio("hello".to_string()),
        )
        .await
        .unwrap();
    let filled: Vec<User> = db.select(users).filter(users.name.eq("Bio")).await.unwrap();
    assert_eq!(filled[0].bio.as_deref(), Some("hello"));
}

#[tokio::test]
async fn generated_sql() {
    let db = besu::dry::<besu_mysql::MySql>();

    let sql = db
        .select(users)
        .filter(users.age.gt(18))
        .filter(users.bio.is_null())
        .order_by(users.name.asc())
        .limit(10)
        .to_sql()
        .unwrap();

    assert_eq!(
        sql.sql(),
        "SELECT `users`.`id`, `users`.`email`, `users`.`name`, `users`.`age`, `users`.`bio` \
         FROM `users` WHERE (`users`.`age` > ?) AND (`users`.`bio` IS NULL) \
         ORDER BY `users`.`name` ASC LIMIT ?"
    );
    assert_eq!(sql.args().len(), 2);

    // MySQL cannot write an `OFFSET` without a `LIMIT`, so an unbounded offset is spelled
    // with a `LIMIT` no result set can reach.
    let sql = db.select(users).offset(20).to_sql().unwrap();
    assert_eq!(
        sql.sql(),
        "SELECT `users`.`id`, `users`.`email`, `users`.`name`, `users`.`age`, `users`.`bio` \
         FROM `users` LIMIT ? OFFSET ?"
    );
    assert_eq!(sql.args()[0], Value::Int(i64::MAX));

    // `NULLS LAST` is emulated rather than written, because MySQL has no such clause.
    let sql = db
        .select(users)
        .order_by(users.bio.as_expr::<()>().nulls_last())
        .to_sql()
        .unwrap();
    assert!(
        sql.sql()
            .ends_with("ORDER BY (`users`.`bio` IS NULL) ASC, `users`.`bio` ASC"),
        "{}",
        sql.sql()
    );
}

#[tokio::test]
async fn generated_ddl() {
    let schema = besu::Database::<Sqlx>::schema([users.def(), posts.def()]);
    let ddl = besu_mysql::MySqlDdl;

    assert_eq!(
        ddl.create_table(schema.table("users").unwrap()),
        "CREATE TABLE `users` (`id` BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY, \
         `email` TEXT NOT NULL, `name` TEXT NOT NULL, `age` INT NOT NULL, `bio` TEXT, \
         UNIQUE KEY `email` (`email`(255))) \
         ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci"
    );

    assert_eq!(
        ddl.create_table(schema.table("posts").unwrap()),
        "CREATE TABLE `posts` (`id` BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY, \
         `title` TEXT NOT NULL, `author_id` BIGINT NOT NULL, \
         CONSTRAINT `posts_author_id_fkey` FOREIGN KEY (`author_id`) \
         REFERENCES `users` (`id`) ON DELETE CASCADE) \
         ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci"
    );

    // MySQL drops an index by naming the table it belongs to.
    assert_eq!(
        ddl.drop_index(
            "users",
            &besu::schema::Index {
                name: "users_email".into(),
                columns: vec!["email".into()],
                unique: true,
            }
        ),
        "DROP INDEX `users_email` ON `users`"
    );

    // And a foreign key by dropping the constraint, without the `CONSTRAINT` keyword.
    assert_eq!(
        ddl.drop_foreign_key(
            "posts",
            &besu::schema::ForeignKey {
                name: "posts_author_id_fkey".into(),
                columns: vec!["author_id".into()],
                references_table: "users".into(),
                references_columns: vec!["id".into()],
                on_delete: besu::schema::ReferenceAction::Cascade,
                on_update: besu::schema::ReferenceAction::Default,
            }
        ),
        "ALTER TABLE `posts` DROP FOREIGN KEY `posts_author_id_fkey`"
    );

    // `MODIFY COLUMN` replaces the whole definition, so the new column is rendered in
    // full.
    let from = besu::schema::Column {
        name: "age".into(),
        ty: TypeDef::Int4,
        nullable: false,
        auto_increment: false,
        primary_key: false,
        unique: false,
        default: None,
        comment: None,
        renamed_from: None,
    };
    let to = besu::schema::Column {
        ty: TypeDef::Int8,
        ..from.clone()
    };
    assert_eq!(
        ddl.alter_column("users", &from, &to),
        ["ALTER TABLE `users` MODIFY COLUMN `age` BIGINT NOT NULL"]
    );
}

#[tokio::test]
async fn filters_and_ordering() {
    let db = setup("besu_test_filters_and_ordering").await;
    for (name, age) in [("Alice", 30), ("Bob", 17), ("Carol", 25)] {
        db.insert(users)
            .value(users.insert(format!("{name}@example.com"), name.to_string(), age))
            .await
            .unwrap();
    }

    let adults: Vec<User> = db
        .select(users)
        .filter(users.age.gte(18))
        .order_by(users.age.desc())
        .await
        .unwrap();
    assert_eq!(
        adults.iter().map(|u| u.name.as_str()).collect::<Vec<_>>(),
        ["Alice", "Carol"]
    );

    let filtered: Vec<User> = db
        .select(users)
        .filter(and(users.age.gte(20), users.name.like("A%")))
        .await
        .unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].name, "Alice");

    let in_list: Vec<User> = db
        .select(users)
        .filter(users.name.in_list(["Alice", "Bob"]))
        .await
        .unwrap();
    assert_eq!(in_list.len(), 2);

    // An `OFFSET` with no `LIMIT` still works, which is what `UNBOUNDED_LIMIT` is for.
    let skipped: Vec<User> = db
        .select(users)
        .order_by(users.age.asc())
        .offset(1)
        .await
        .unwrap();
    assert_eq!(
        skipped.iter().map(|u| u.name.as_str()).collect::<Vec<_>>(),
        ["Carol", "Alice"]
    );
}

#[tokio::test]
async fn projection_and_aggregates() {
    let db = setup("besu_test_projection_and_aggregates").await;
    for (name, age) in [("Alice", 30), ("Bob", 20)] {
        db.insert(users)
            .value(users.insert(format!("{name}@example.com"), name.to_string(), age))
            .await
            .unwrap();
    }

    let names: Vec<(i64, String)> = db.select(users).pick((users.id, users.name)).await.unwrap();
    assert_eq!(names.len(), 2);

    let counts: Vec<i64> = db.select(users).pick(count_all()).await.unwrap();
    assert_eq!(counts, [2]);

    // `SUM` over an integer is a `DECIMAL`, which Besu does not decode, so the query casts
    // it to a float first.
    let ages: Vec<f64> = db
        .select(users)
        .pick(sum(users.age.as_expr().cast::<besu::Float8>()))
        .await
        .unwrap();
    assert_eq!(ages, [50.0]);
}

#[tokio::test]
async fn update_and_delete() {
    let db = setup("besu_test_update_and_delete").await;
    db.insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .await
        .unwrap();

    let updated = db
        .update(users)
        .set(users.name, "Alicia")
        .filter(users.name.eq("Alice"))
        .await
        .unwrap();
    assert_eq!(updated.rows_affected(), 1);

    let remaining: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(remaining[0].name, "Alicia");
    assert_eq!(remaining[0].id, 1);

    let deleted = db
        .delete(users)
        .filter(users.name.eq("Alicia"))
        .await
        .unwrap();
    assert_eq!(deleted.rows_affected(), 1);

    let remaining: Vec<User> = db.select(users).await.unwrap();
    assert!(remaining.is_empty());
}

#[tokio::test]
async fn joins() {
    let db = setup("besu_test_joins").await;
    db.insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .await
        .unwrap();
    let author: User = db.select(users).await.unwrap().remove(0);
    db.insert(posts)
        .value(posts.insert("Hello", author.id))
        .await
        .unwrap();

    let rows: Vec<(User, Option<Post>)> = db
        .select(users)
        .left_join(posts, eq(users.id, posts.author_id))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0.name, "Alice");
    assert_eq!(rows[0].1.as_ref().unwrap().title, "Hello");

    let rows: Vec<(User, Post)> = db
        .select(users)
        .inner_join(posts, eq(users.id, posts.author_id))
        .filter(posts.title.eq("Hello"))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);

    // A user with no post produces one row of `NULL`s on the right of a left join.
    db.insert(users)
        .value(users.insert("b@example.com", "Bob", 40))
        .await
        .unwrap();
    let rows: Vec<(User, Option<Post>)> = db
        .select(users)
        .left_join(posts, eq(users.id, posts.author_id))
        .order_by(users.name.asc())
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0.name, "Alice");
    assert!(rows[0].1.is_some());
    assert_eq!(rows[1].0.name, "Bob");
    assert!(rows[1].1.is_none());
}

#[tokio::test]
async fn upsert() {
    let db = setup("besu_test_upsert").await;
    db.insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .await
        .unwrap();

    // MySQL has no `ON CONFLICT (...) DO UPDATE`; `ON DUPLICATE KEY UPDATE` fires on any
    // unique key, so the target is ignored.
    db.insert(users)
        .value(users.insert("a@example.com", "Alice", 31))
        .on_conflict(users.email)
        .do_update(users.age, 31)
        .await
        .unwrap();

    let rows: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].age, 31);
}

#[tokio::test]
async fn transactions() {
    let db = setup("besu_test_transactions").await;

    db.transaction(async |tx| {
        tx.insert(users)
            .value(users.insert("a@example.com", "Alice", 30))
            .await?;
        Ok(())
    })
    .await
    .unwrap();

    let count: i64 = db.select(users).pick(count_all()).await.unwrap().remove(0);
    assert_eq!(count, 1);
}

#[tokio::test]
async fn raw_sql() {
    let db = setup("besu_test_raw_sql").await;
    db.insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .await
        .unwrap();

    let rows: Vec<User> = db
        .execute(sql!("SELECT * FROM users WHERE age > ?", 18))
        .all()
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);

    let age = db
        .execute(sql!("SELECT age FROM users WHERE name = ?", "Alice"))
        .all::<(i32,)>()
        .await
        .unwrap()
        .remove(0)
        .0;
    assert_eq!(age, 30);
}

#[tokio::test]
async fn bulk_update_requires_intent() {
    let db = setup("besu_test_bulk_update_requires_intent").await;
    // `.all_rows()` is the only way to run an update with no `WHERE`; without it this
    // would not compile.
    db.update(users)
        .set(users.age, 40)
        .all_rows()
        .await
        .unwrap();
}

#[tokio::test]
async fn returning_is_rejected() {
    let db = setup("besu_test_returning_is_rejected").await;

    // The query builder records the problem while rendering and the driver turns it into
    // its own error type, so it surfaces when the query is awaited rather than at build
    // time.
    let err = db
        .insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .returning(users)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unsupported"), "{err}");
    assert!(err.to_string().contains("RETURNING"), "{err}");

    let err = db
        .update(users)
        .set(users.age, 31)
        .filter(users.id.eq(1))
        .returning(users.id)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unsupported"), "{err}");

    let err = db
        .delete(users)
        .filter(users.id.eq(1))
        .returning(users.id)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unsupported"), "{err}");
}

#[tokio::test]
async fn binds_and_decodes_values() {
    let db = fresh("besu_test_binds_and_decodes_values").await;

    // A `SELECT` of a parameter round-trips through MySQL's own type inference, so this
    // checks encoding and decoding rather than table storage.
    let rows: Vec<(bool,)> = db.execute(one("SELECT ?", true)).all().await.unwrap();
    assert!(rows[0].0);

    let id = uuid::Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
    let rows: Vec<(uuid::Uuid,)> = db.execute(one("SELECT ?", id)).all().await.unwrap();
    assert_eq!(rows[0].0, id);

    let json = serde_json::json!({"name": "Oscar", "age": 21});
    let rows: Vec<(serde_json::Value,)> = db
        .execute(one("SELECT CAST(? AS JSON)", json.clone()))
        .all()
        .await
        .unwrap();
    assert_eq!(rows[0].0, json);

    let timestamp = chrono::NaiveDate::from_ymd_opt(2026, 11, 12)
        .and_then(|date| date.and_hms_opt(9, 30, 0))
        .unwrap();
    let rows: Vec<(chrono::NaiveDateTime,)> = db
        .execute(one("SELECT CAST(? AS DATETIME(6))", timestamp))
        .all()
        .await
        .unwrap();
    assert_eq!(rows[0].0, timestamp);

    // MySQL has real unsigned integers, so the top of the `u64` range survives.
    let rows: Vec<(u64,)> = db
        .execute(one("SELECT CAST(? AS UNSIGNED)", u64::MAX))
        .all()
        .await
        .unwrap();
    assert_eq!(rows[0].0, u64::MAX);
}

#[tokio::test]
async fn schema_push_converges() {
    let db = fresh("besu_test_schema_push_converges").await;

    let first = db.push([users.def(), posts.def()]).await.unwrap();
    assert_eq!(first.changes.len(), 2);
    assert!(first.statements[0].contains("CREATE TABLE `users`"));
    assert!(
        first
            .statements
            .iter()
            .any(|s| s.contains("AUTO_INCREMENT")),
        "auto-incrementing primary keys must be inline in MySQL: {:?}",
        first.statements
    );
    assert!(
        first.statements.iter().all(
            |s| s.ends_with("ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci")
        ),
        "every table should be created as utf8mb4 InnoDB: {:?}",
        first.statements
    );

    // Pushing the same schema again must be a no-op: that is the property that makes
    // `push` safe to run on every boot.
    let second = db.push([users.def(), posts.def()]).await.unwrap();
    assert!(
        second.is_empty(),
        "second push should be a no-op, got {:?}",
        second.changes
    );

    // And the introspected schema must round-trip exactly.
    let introspected = db.introspect().await.unwrap();
    let expected = besu::Database::<Sqlx>::schema([users.def(), posts.def()]);
    assert_eq!(introspected, expected);

    let users_table = introspected.table("users").unwrap();
    assert_eq!(users_table.columns.len(), 5);
    assert!(
        users_table
            .columns
            .iter()
            .any(|c| c.name == "id" && c.auto_increment && c.primary_key)
    );
    assert!(
        users_table
            .columns
            .iter()
            .any(|c| c.name == "email" && c.unique && !c.nullable)
    );
    assert!(
        users_table
            .columns
            .iter()
            .any(|c| c.name == "bio" && c.nullable)
    );
    assert!(users_table.indexes.is_empty(), "{:?}", users_table.indexes);

    let posts_table = introspected.table("posts").unwrap();
    assert_eq!(posts_table.foreign_keys.len(), 1);
    assert_eq!(posts_table.foreign_keys[0].name, "posts_author_id_fkey");
    assert_eq!(posts_table.foreign_keys[0].references_table, "users");
    assert_eq!(
        posts_table.foreign_keys[0].on_delete,
        besu::schema::ReferenceAction::Cascade
    );
    // The index InnoDB created for the foreign key must not leak into the schema.
    assert!(posts_table.indexes.is_empty(), "{:?}", posts_table.indexes);
}

#[tokio::test]
async fn secondary_indexes_round_trip() {
    let db = fresh("besu_test_secondary_indexes").await;

    let first = db.push([tags.def()]).await.unwrap();
    assert_eq!(first.changes.len(), 1);
    assert!(
        first.statements[0].contains("KEY `tags_name_idx` (`name`(255))"),
        "a `TEXT` index needs a prefix length: {:?}",
        first.statements
    );

    // The index must not look like drift on the next push.
    let second = db.push([tags.def()]).await.unwrap();
    assert!(second.is_empty(), "{:?}", second.changes);

    let introspected = db.introspect().await.unwrap();
    assert_eq!(introspected, besu::Database::<Sqlx>::schema([tags.def()]));

    let table = introspected.table("tags").unwrap();
    assert_eq!(table.indexes.len(), 1);
    assert_eq!(table.indexes[0].name, "tags_name_idx");
    assert_eq!(table.indexes[0].columns, ["name"]);
    assert!(!table.indexes[0].unique);
}
