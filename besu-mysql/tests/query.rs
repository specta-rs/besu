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

use besu::Driver;
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
                kind: besu::IndexKind::Btree,
                predicate: None,
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
        generated: None,
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

#[tokio::test]
async fn json_path_access_renders_and_reads() {
    let db = fresh("besu_json_path").await;

    // `JSON_EXTRACT` returns a JSON value, so the text form wraps it in `JSON_UNQUOTE` —
    // which is exactly what `json_text` renders.
    let rows: Vec<(String,)> = db
        .execute(sql!(
            "SELECT JSON_UNQUOTE(JSON_EXTRACT(?, '$.user.name')) AS name",
            r#"{"user": {"name": "Oscar", "age": 21}}"#
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(rows, [("Oscar".to_string(),)]);

    // The typed helpers render MySQL's spelling, with `JSON_UNQUOTE` only for the text form.
    let rendered = besu::dry::<besu_mysql::MySql>();
    let sql = rendered
        .execute(sql!("SELECT JSON_UNQUOTE(JSON_EXTRACT(?, '$.a'))", ""))
        .to_sql()
        .unwrap();
    assert!(sql.sql().contains("JSON_EXTRACT"), "got {}", sql.sql());
}

/// MySQL only matches through a `FULLTEXT` index over exactly the columns named, so the
/// index is part of the set-up rather than something the expression can imply.
#[tokio::test]
async fn full_text_match() {
    let db = setup("besu_test_full_text").await;
    db.execute(sql!(
        "ALTER TABLE posts ADD FULLTEXT INDEX posts_title_fts (title)"
    ))
    .await
    .unwrap();

    db.insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .await
        .unwrap();
    let author: User = db.select(users).await.unwrap().remove(0);
    for title in ["Rust ownership and borrowing", "Gardening in spring"] {
        db.insert(posts)
            .value(posts.insert(title, author.id))
            .await
            .unwrap();
    }

    let found: Vec<Post> = db
        .select(posts)
        .filter(posts.title.matches("borrowing"))
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].title, "Rust ownership and borrowing");

    // An unrelated word matches nothing, which is what a boolean-mode match promises.
    let unrelated: Vec<Post> = db
        .select(posts)
        .filter(matches(posts.title, "compiler"))
        .await
        .unwrap();
    assert!(unrelated.is_empty());
}

#[tokio::test]
async fn full_text_sql() {
    let db = besu::dry::<besu_mysql::MySql>();
    let sql = db
        .select(posts)
        .filter(posts.title.matches("rust"))
        .to_sql()
        .unwrap();

    // MySQL has no `RETURNING` and no cache-key suffix, so the statement is exactly one
    // placeholder and no trailing comment.
    assert_eq!(
        sql.sql(),
        "SELECT `posts`.`id`, `posts`.`title`, `posts`.`author_id` FROM `posts` \
         WHERE MATCH(`posts`.`title`) AGAINST (? IN BOOLEAN MODE)"
    );
    assert_eq!(sql.args(), [Value::Text("rust".into())]);
}

/// The names in row order, so an ordering assertion reads as the list it checks.
fn names(rows: &[User]) -> Vec<&str> {
    rows.iter().map(|user| user.name.as_str()).collect()
}

/// Set operations are one statement: each branch keeps its own filter, the operator decides
/// which rows survive, and the compound's `ORDER BY` sees the whole result.
#[tokio::test]
async fn set_operations() {
    let db = setup("besu_test_set_operations").await;
    for (email, name, age) in [
        ("alice@example.com", "Alice", 30),
        ("bob@example.com", "Bob", 17),
        ("carol@example.com", "Carol", 25),
        ("dave@example.com", "Dave", 40),
    ] {
        db.insert(users)
            .value(users.insert(email, name, age))
            .await
            .unwrap();
    }

    let everyone: Vec<User> = db
        .select(users)
        .filter(users.age.gte(18))
        .union(db.select(users).filter(users.age.lt(18)))
        .order_by(users.name.asc())
        .all()
        .await
        .unwrap();
    assert_eq!(names(&everyone), ["Alice", "Bob", "Carol", "Dave"]);

    let distinct: Vec<User> = db
        .select(users)
        .filter(users.age.gt(20))
        .union(db.select(users).filter(users.age.gt(35)))
        .order_by(users.name.asc())
        .all()
        .await
        .unwrap();
    assert_eq!(names(&distinct), ["Alice", "Carol", "Dave"]);

    let all: Vec<User> = db
        .select(users)
        .filter(users.age.gt(20))
        .union_all(db.select(users).filter(users.age.gt(35)))
        .order_by(users.name.asc())
        .all()
        .await
        .unwrap();
    assert_eq!(names(&all), ["Alice", "Carol", "Dave", "Dave"]);

    let both: Vec<User> = db
        .select(users)
        .filter(users.age.gt(20))
        .intersect(db.select(users).filter(users.name.like("C%")))
        .all()
        .await
        .unwrap();
    assert_eq!(names(&both), ["Carol"]);

    let only: Vec<User> = db
        .select(users)
        .filter(users.age.gte(18))
        .except(db.select(users).filter(users.name.like("C%")))
        .order_by(users.name.asc())
        .all()
        .await
        .unwrap();
    assert_eq!(names(&only), ["Alice", "Dave"]);

    // Ordering, paging and `first` belong to the compound.
    let paged: Vec<User> = db
        .select(users)
        .filter(users.age.gte(18))
        .union(db.select(users).filter(users.age.lt(18)))
        .order_by_desc(users.age.asc())
        .limit(2)
        .offset(1)
        .all()
        .await
        .unwrap();
    assert_eq!(names(&paged), ["Alice", "Carol"]);

    let first: Option<User> = db
        .select(users)
        .filter(users.age.gte(18))
        .union(db.select(users).filter(users.age.lt(18)))
        .order_by(users.age.asc())
        .first()
        .await
        .unwrap();
    assert_eq!(first.map(|user| user.name), Some("Bob".to_string()));
}

/// The `ALL` operators are multiset operations, which a plain set operator cannot express:
/// two copies intersected with two copies stay two copies, and subtraction is by count.
#[tokio::test]
async fn the_all_operators_keep_counts() {
    let db = setup("besu_test_set_operations_all").await;
    for (email, name, age) in [
        ("alice1@example.com", "Alice", 30),
        ("alice2@example.com", "Alice", 30),
        ("bob@example.com", "Bob", 17),
    ] {
        db.insert(users)
            .value(users.insert(email, name, age))
            .await
            .unwrap();
    }

    let doubled: Vec<String> = db
        .select(users)
        .pick(users.name)
        .intersect_all(db.select(users).pick(users.name))
        .order_by(users.name.asc())
        .all()
        .await
        .unwrap();
    assert_eq!(doubled, ["Alice", "Alice", "Bob"]);

    let singles: Vec<String> = db
        .select(users)
        .pick(users.name)
        .intersect(db.select(users).pick(users.name))
        .order_by(users.name.asc())
        .all()
        .await
        .unwrap();
    assert_eq!(singles, ["Alice", "Bob"]);

    // `EXCEPT ALL` removes one copy of `Alice`, `EXCEPT` removes both.
    let all: Vec<String> = db
        .select(users)
        .pick(users.name)
        .except_all(
            db.select(users)
                .filter(users.email.eq("alice1@example.com"))
                .pick(users.name),
        )
        .order_by(users.name.asc())
        .all()
        .await
        .unwrap();
    assert_eq!(all, ["Alice", "Bob"]);

    let distinct: Vec<String> = db
        .select(users)
        .pick(users.name)
        .except(
            db.select(users)
                .filter(users.email.eq("alice1@example.com"))
                .pick(users.name),
        )
        .order_by(users.name.asc())
        .all()
        .await
        .unwrap();
    assert_eq!(distinct, ["Bob"]);
}

/// `exists` and `in_query` are subqueries of the enclosing statement: their parameters are
/// part of the same numbering and their rows are not decoded separately.
#[tokio::test]
async fn subqueries() {
    let db = setup("besu_test_subqueries").await;
    for (email, name, age) in [
        ("alice@example.com", "Alice", 30),
        ("bob@example.com", "Bob", 17),
        ("carol@example.com", "Carol", 25),
    ] {
        db.insert(users)
            .value(users.insert(email, name, age))
            .await
            .unwrap();
    }
    let alice = db
        .select(users)
        .filter(users.name.eq("Alice"))
        .first()
        .await
        .unwrap()
        .unwrap();
    db.insert(posts)
        .values([
            posts.insert("Hello", alice.id),
            posts.insert("World", alice.id),
        ])
        .await
        .unwrap();

    let with_posts: Vec<User> = db
        .select(users)
        .filter(exists(
            db.select(posts)
                .correlate(users)
                .filter(posts.author_id.eq(users.id)),
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(names(&with_posts), ["Alice"]);

    let without_posts: Vec<User> = db
        .select(users)
        .filter(not_exists(
            db.select(posts)
                .correlate(users)
                .filter(posts.author_id.eq(users.id)),
        ))
        .order_by(users.name.asc())
        .all()
        .await
        .unwrap();
    assert_eq!(names(&without_posts), ["Bob", "Carol"]);

    let hello: Vec<User> = db
        .select(users)
        .filter(exists(
            db.select(posts)
                .correlate(users)
                .filter(posts.author_id.eq(users.id))
                .filter(posts.title.eq("Hello")),
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(names(&hello), ["Alice"]);

    let authors: Vec<User> = db
        .select(users)
        .filter(users.id.in_query(db.select(posts).pick(posts.author_id)))
        .all()
        .await
        .unwrap();
    assert_eq!(names(&authors), ["Alice"]);

    let non_authors: Vec<User> = db
        .select(users)
        .filter(
            users
                .id
                .not_in_query(db.select(posts).pick(posts.author_id)),
        )
        .order_by(users.name.asc())
        .all()
        .await
        .unwrap();
    assert_eq!(names(&non_authors), ["Bob", "Carol"]);
}

/// The exact statement MySQL receives for a compound query and for each subquery form.
#[tokio::test]
async fn set_operation_and_subquery_sql() {
    let db = besu::dry::<besu_mysql::MySql>();

    let sql = db
        .select(users)
        .pick(users.id)
        .filter(users.age.gte(18))
        .union(db.select(users).pick(users.id).filter(users.age.lt(18)))
        .order_by(users.name.asc())
        .limit(10)
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        "SELECT `users`.`id` FROM `users` WHERE (`users`.`age` >= ?) UNION SELECT `users`.`id` FROM `users` WHERE (`users`.`age` < ?) ORDER BY `name` ASC LIMIT ?"
    );
    assert_eq!(sql.args(), [Value::Int(18), Value::Int(18), Value::Int(10)]);

    let sql = db
        .select(users)
        .pick(users.id)
        .filter(exists(
            db.select(posts)
                .correlate(users)
                .filter(posts.author_id.eq(users.id)),
        ))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        "SELECT `users`.`id` FROM `users` WHERE EXISTS (SELECT `posts`.`id`, `posts`.`title`, `posts`.`author_id` FROM `posts` WHERE (`posts`.`author_id` = `users`.`id`))"
    );
    assert!(sql.args().is_empty());

    let sql = db
        .select(users)
        .pick(users.id)
        .filter(users.id.in_query(db.select(posts).pick(posts.author_id)))
        .to_sql()
        .unwrap();
    assert_eq!(
        sql.sql(),
        "SELECT `users`.`id` FROM `users` WHERE (`users`.`id` IN (SELECT `posts`.`author_id` FROM `posts`))"
    );
}

/// MySQL 8.0.19 reads the row the insert is trying to add through a row alias, so
/// `excluded` and `new_row` both render `new.column` here. `do_nothing` has no MySQL
/// spelling, so it becomes a self-assignment on the conflict target.
#[tokio::test]
async fn upsert_from_the_incoming_row() {
    let db = setup("besu_test_upsert_row_alias").await;
    db.insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .await
        .unwrap();

    db.insert(users)
        .value(users.insert("a@example.com", "Alice Updated", 31))
        .on_conflict(users.email)
        .do_update(users.name, new_row(users.name))
        .do_update(users.age, excluded(users.age))
        .await
        .unwrap();

    let rows: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "Alice Updated");
    assert_eq!(rows[0].age, 31);

    // The alias is chosen per statement, and the custom name is used at render time.
    db.insert(users)
        .value(users.insert("a@example.com", "Alice", 40))
        .row_alias("incoming")
        .on_conflict(users.email)
        .do_update(users.age, new_row(users.age))
        .await
        .unwrap();

    let rows: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(rows[0].name, "Alice Updated");
    assert_eq!(rows[0].age, 40);

    // `do_nothing` assigns the conflict target to itself, which leaves the row alone
    // without the error-swallowing of `INSERT IGNORE`.
    db.insert(users)
        .value(users.insert("a@example.com", "Nope", 1))
        .on_conflict(users.email)
        .do_nothing()
        .await
        .unwrap();

    let rows: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "Alice Updated");
    assert_eq!(rows[0].age, 40);
}

/// An empty `IN` list is a syntax error, so it renders as a constant predicate instead.
#[tokio::test]
async fn empty_in_lists_execute() {
    let db = setup("besu_test_empty_in_lists").await;
    db.insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .await
        .unwrap();

    let none: Vec<User> = db
        .select(users)
        .filter(users.id.in_list(Vec::<i64>::new()))
        .await
        .unwrap();
    assert!(none.is_empty());

    let all: Vec<User> = db
        .select(users)
        .filter(users.id.not_in(Vec::<i64>::new()))
        .await
        .unwrap();
    assert_eq!(all.len(), 1);
}

/// Every `SqlType` marker the crate ships has to survive a trip through `CAST`. MySQL is
/// the dialect whose `CAST` grammar is a closed list, so this is where a DDL spelling would
/// have been a syntax error.
#[tokio::test]
async fn cast_to_every_marker_executes() {
    let db = setup("besu_test_cast_markers").await;
    db.insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .await
        .unwrap();

    // The results are not decoded: the point is that the server accepts the statement, so
    // the SQL is executed through the driver directly.
    macro_rules! cast {
        ($literal:literal, $ty:ty) => {{
            let sql = db
                .select(users)
                .pick(raw::<(UserTable,)>($literal).cast::<$ty>())
                .to_sql()
                .unwrap();
            db.driver().query(&sql).await.unwrap();
        }};
    }

    cast!("1", besu::Bool);
    cast!("1", besu::Int1);
    cast!("1", besu::Int2);
    cast!("1", besu::Int4);
    cast!("1", besu::Int8);
    cast!("1", besu::UInt1);
    cast!("1", besu::UInt2);
    cast!("1", besu::UInt4);
    cast!("1", besu::UInt8);
    cast!("1", besu::U128);
    cast!("1", besu::I128);
    cast!("1.5", besu::Float4);
    cast!("1.5", besu::Float8);
    cast!("'x'", besu::Text);
    cast!("'x'", besu::Char<8>);
    cast!("'x'", besu::VarChar<8>);
    cast!("'x'", besu::Bytes);
    cast!("'00000000-0000-0000-0000-000000000000'", besu::Uuid);
    cast!("'{}'", besu::Json);
    cast!("'{}'", besu::Jsonb);
    cast!("'2020-01-01'", besu::Date);
    cast!("'12:00:00'", besu::Time);
    cast!("'2020-01-01 12:00:00'", besu::Timestamp);
    cast!("'2020-01-01 12:00:00'", besu::TimestampTz);
    cast!("1.5", besu::Decimal<10, 2>);
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "documents")]
pub struct Document {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub payload: serde_json::Value,
}

/// The typed JSON helpers are rendered, not just the raw SQL they were modelled on. The
/// MySQL spelling was a syntax error for a while because a test asserted its own input.
#[tokio::test]
async fn typed_json_paths_render_and_execute() {
    let db = fresh("besu_test_typed_json").await;
    db.push([documents.def()]).await.unwrap();
    db.insert(documents)
        .value(documents.insert(serde_json::json!({ "user": { "name": "Oscar", "age": 21 } })))
        .await
        .unwrap();

    let text: Vec<String> = db
        .select(documents)
        .pick(besu::json_text(documents.payload, "user.name"))
        .all()
        .await
        .unwrap();
    assert_eq!(text, ["Oscar".to_string()]);

    let value: Vec<serde_json::Value> = db
        .select(documents)
        .pick(besu::json_get(documents.payload, "user"))
        .all()
        .await
        .unwrap();
    assert_eq!(value[0], serde_json::json!({ "name": "Oscar", "age": 21 }));
}

// ---------------------------------------------------------------------------------------
// Comments, operators, functions and common table expressions
// ---------------------------------------------------------------------------------------

/// A table with dates and a boolean, so the date and aggregate functions have something to
/// work on that `users` and `posts` do not provide.
#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "notes")]
pub struct Note {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub body: String,
    pub created: chrono::NaiveDate,
    pub stamp: chrono::NaiveDateTime,
    pub active: bool,
}

/// A self-referencing table, so a recursive expression has something to walk.
#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "nodes")]
pub struct Node {
    #[besu(primary_key)]
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
}

async fn setup_notes(name: &str) -> besu::Database<Sqlx> {
    let db = fresh(name).await;
    db.push([users.def(), posts.def(), notes.def(), nodes.def()])
        .await
        .unwrap();
    db
}

fn date(year: i32, month: u32, day: u32) -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn stamp(year: i32, month: u32, day: u32) -> chrono::NaiveDateTime {
    date(year, month, day).and_hms_opt(12, 0, 0).unwrap()
}

#[tokio::test]
async fn comments_are_part_of_the_statement() {
    let db = setup("besu_test_comments").await;

    let sql = db.select(users).comment("feed page").to_sql().unwrap();
    assert!(sql.sql().ends_with("/* feed page */"), "{}", sql.sql());

    let rows: Vec<User> = db.select(users).comment("feed page").all().await.unwrap();
    assert!(rows.is_empty());
}

/// MySQL's null-safe comparison is `<=>`, and `IS NOT DISTINCT FROM` is that operator
/// without the negation. Its regular expressions are `REGEXP` and `REGEXP_LIKE`.
#[tokio::test]
async fn null_safe_comparisons_and_regexes_run() {
    let db = setup("besu_test_null_safe").await;
    db.insert(users)
        .value(users.insert("a@example.com", "Alice", 20))
        .await
        .unwrap();
    db.insert(users)
        .value(
            users
                .insert("b@example.com", "alice", 30)
                .bio("hello".to_string()),
        )
        .await
        .unwrap();

    let differing: Vec<User> = db
        .select(users)
        .filter(users.bio.is_distinct_from("hello"))
        .all()
        .await
        .unwrap();
    assert_eq!(differing.len(), 1);
    assert_eq!(differing[0].name, "Alice");

    let same: Vec<User> = db
        .select(users)
        .filter(users.bio.is_not_distinct_from(Option::<String>::None))
        .all()
        .await
        .unwrap();
    assert_eq!(same.len(), 1);

    // MySQL's `REGEXP` follows the column's collation, and the default is
    // `utf8mb4_unicode_ci`, so even the nominally case-sensitive form matches both names.
    // `REGEXP_LIKE(.., 'i')` is the explicit spelling of the same thing.
    let matched: Vec<User> = db
        .select(users)
        .filter(users.name.regex_match("^A"))
        .all()
        .await
        .unwrap();
    assert_eq!(matched.len(), 2);

    let insensitive: Vec<User> = db
        .select(users)
        .filter(users.name.regex_match_ci("^A"))
        .all()
        .await
        .unwrap();
    assert_eq!(insensitive.len(), 2);

    let negated: Vec<User> = db
        .select(users)
        .filter(users.name.not_regex_match_ci("^A"))
        .all()
        .await
        .unwrap();
    assert!(negated.is_empty());
}

/// The string helpers MySQL spells differently: `SUBSTRING_INDEX` for `split_part`,
/// `INSTR` for `strpos`, `SHA2(x, 256)` for `sha256`.
#[tokio::test]
async fn string_functions_run() {
    let db = setup("besu_test_string_fns").await;
    db.insert(users)
        .value(users.insert("a@example.com", "Alice", 20))
        .await
        .unwrap();

    let rows: Vec<(i64, i64, String, String, String, String, String)> = db
        .select(users)
        .pick((
            char_length(users.name),
            octet_length(users.name),
            substr(users.name, 2, 3),
            left(users.name, 2),
            right(users.name, 2),
            lpad(users.name, 7, "."),
            rpad(users.name, 7, "."),
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(
        rows[0],
        (
            5,
            5,
            "lic".to_string(),
            "Al".to_string(),
            "ce".to_string(),
            "..Alice".to_string(),
            "Alice..".to_string(),
        )
    );

    let rows: Vec<(String, i64, String, String)> = db
        .select(users)
        .pick((
            concat_ws("-", users.name, "x"),
            strpos(users.name, "ice"),
            md5(users.name),
            sha256(users.name),
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(rows[0].0, "Alice-x");
    assert_eq!(rows[0].1, 3);
    assert_eq!(rows[0].2, "64489c85dc2fe0787b85cd87214b3810");
    assert_eq!(
        rows[0].3,
        "3bc51062973c458d5a6f2d8d64a023246354ad7e064b1e4e009ec8a0699a3043"
    );

    // `split_part` is PostgreSQL's, and MySQL's `SUBSTRING_INDEX` is not the same function:
    // it returns everything before the nth delimiter rather than the nth field.
    let error = db
        .select(users)
        .pick(split_part("a,b,c", ",", 2))
        .all()
        .await
        .unwrap_err();
    assert!(error.to_string().contains("split_part"), "{error}");

    let rows: Vec<(String, String, String)> = db
        .select(users)
        .pick((
            regexp_replace(users.name, "A", "O"),
            regexp_substr(users.name, "l.*e"),
            to_base64(users.name),
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(rows[0].0, "Olice");
    assert_eq!(rows[0].1, "lice");
    assert_eq!(rows[0].2, "QWxpY2U=");

    let decoded: Vec<String> = db
        .select(users)
        .pick(from_base64(to_base64(users.name)))
        .all()
        .await
        .unwrap();
    assert_eq!(decoded, ["Alice"]);
}

/// The date helpers MySQL spells differently: `DATE_FORMAT`, `DATE_ADD` with one unit per
/// interval, and `FROM_UNIXTIME`.
#[tokio::test]
async fn date_functions_run() {
    let db = setup_notes("besu_test_date_fns").await;
    db.insert(notes)
        .value(notes.insert(
            "first".to_string(),
            date(2024, 3, 15),
            stamp(2024, 3, 15),
            true,
        ))
        .await
        .unwrap();

    let rows: Vec<(i64, i64, i64, i64, String)> = db
        .select(notes)
        .pick((
            extract(DateField::Year, notes.created),
            extract(DateField::Month, notes.created),
            datediff(notes.created, notes.created),
            extract(DateField::Epoch, notes.stamp),
            date_format(notes.created, "%Y-%m"),
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(rows[0].0, 2024);
    assert_eq!(rows[0].1, 3);
    assert_eq!(rows[0].2, 0);
    assert!(rows[0].3 > 1_700_000_000);
    assert_eq!(rows[0].4, "2024-03");

    let shifted: Vec<chrono::NaiveDateTime> = db
        .select(notes)
        .pick(date_add(notes.stamp, Interval::new().months(1).days(2)))
        .all()
        .await
        .unwrap();
    assert_eq!(shifted, [stamp(2024, 4, 17)]);

    let unix: Vec<chrono::NaiveDateTime> = db
        .select(notes)
        .pick(from_unixtime(unix_timestamp(notes.stamp)))
        .all()
        .await
        .unwrap();
    assert_eq!(unix, [stamp(2024, 3, 15)]);

    let today: Vec<chrono::NaiveDate> = db.select(notes).pick(current_date()).all().await.unwrap();
    assert_eq!(today.len(), 1);
}

/// `string_agg` is `GROUP_CONCAT(x SEPARATOR y)` here, and MySQL has no `FILTER` clause at
/// all, so that one records an error rather than dropping the condition.
#[tokio::test]
async fn aggregate_functions_run() {
    let db = setup_notes("besu_test_aggregates").await;
    db.insert(notes)
        .values([
            notes.insert("a".to_string(), date(2024, 1, 1), stamp(2024, 1, 1), true),
            notes.insert("b".to_string(), date(2024, 1, 2), stamp(2024, 1, 2), false),
        ])
        .await
        .unwrap();

    let rows: Vec<(String, bool, bool, serde_json::Value)> = db
        .select(notes)
        .pick((
            string_agg(notes.body, ","),
            bool_and(notes.active),
            bool_or(notes.active),
            json_agg(notes.body),
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(rows[0].0, "a,b");
    assert!(!rows[0].1);
    assert!(rows[0].2);
    assert_eq!(rows[0].3, serde_json::json!(["a", "b"]));

    let error = db
        .select(notes)
        .pick(count_all().filter(notes.active))
        .all()
        .await
        .unwrap_err();
    assert!(error.to_string().contains("FILTER"), "{error}");
}

/// MySQL supports window functions but not `FILTER`, so the window half runs and the filter
/// half is refused.
#[tokio::test]
async fn windows_run() {
    let db = setup_notes("besu_test_windows").await;
    db.insert(notes)
        .values([
            notes.insert("a".to_string(), date(2024, 1, 1), stamp(2024, 1, 1), true),
            notes.insert("b".to_string(), date(2024, 1, 2), stamp(2024, 1, 2), false),
            notes.insert("c".to_string(), date(2024, 1, 3), stamp(2024, 1, 3), true),
        ])
        .await
        .unwrap();

    let rows: Vec<(String, i64, i64)> = db
        .select(notes)
        .pick((
            notes.body,
            row_number().over(Window::new().order_by(notes.id.asc())),
            dense_rank().over(Window::new().order_by(notes.active.asc())),
        ))
        .order_by(notes.id.asc())
        .all()
        .await
        .unwrap();
    assert_eq!(rows[0], ("a".to_string(), 1, 2));
    assert_eq!(rows[1], ("b".to_string(), 2, 1));
    assert_eq!(rows[2].1, 3);
}

/// An expression is defined where it is used, and a recursive one walks the tree it defines.
#[tokio::test]
async fn common_table_expressions_run() {
    let db = setup_notes("besu_test_ctes").await;

    db.insert(users)
        .values([
            users.insert("a@example.com", "A", 20),
            users.insert("b@example.com", "B", 30),
            users.insert("c@example.com", "C", 10),
        ])
        .await
        .unwrap();

    let adults = db.select(users).filter(users.age.gte(18)).cte("adults");
    let rows: Vec<User> = db
        .from(&adults)
        .filter(adults.column(users.age).lt(25))
        .all()
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "A");

    db.insert(nodes)
        .value(nodes.insert(1, "root".to_string()))
        .await
        .unwrap();
    db.insert(nodes)
        .value(nodes.insert(2, "child".to_string()).parent_id(Some(1_i64)))
        .await
        .unwrap();
    db.insert(nodes)
        .value(
            nodes
                .insert(3, "grandchild".to_string())
                .parent_id(Some(2_i64)),
        )
        .await
        .unwrap();

    let tree = db
        .select(nodes)
        .filter(nodes.parent_id.is_null())
        .cte("tree");
    let rows: Vec<Node> = tree
        .clone()
        .union_all(
            db.select(nodes)
                .inner_join_cte(&tree, nodes.parent_id.eq(tree.column(nodes.id)))
                .pick(nodes),
        )
        .order_by(nodes.id.asc())
        .all()
        .await
        .unwrap();
    assert_eq!(
        rows.iter()
            .map(|node| node.name.as_str())
            .collect::<Vec<_>>(),
        ["root", "child", "grandchild"]
    );
}

/// The numeric helpers, including the mathematical functions MySQL spells directly.
#[tokio::test]
async fn numeric_functions_run() {
    let db = setup("besu_test_numeric_fns").await;
    db.insert(users)
        .value(users.insert("a@example.com", "A", 20))
        .await
        .unwrap();

    let double = || users.age.as_expr().cast::<besu::Float8>();
    let rows: Vec<(f64, f64, f64, f64, f64, f64)> = db
        .select(users)
        .pick((
            sqrt(double()),
            power(double(), 2),
            ln(double()),
            log10(double()),
            exp(ln(double())),
            log(2.0, double()),
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(rows[0].0, 20.0_f64.sqrt());
    assert_eq!(rows[0].1, 400.0);
    assert!((rows[0].2 - 20.0_f64.ln()).abs() < 1e-9);
    assert!((rows[0].3 - 20.0_f64.log10()).abs() < 1e-9);
    assert!((rows[0].4 - 20.0).abs() < 1e-9);
    assert!((rows[0].5 - 20.0_f64.log2()).abs() < 1e-9);

    let rows: Vec<(f64, f64, f64, i64)> = db
        .select(users)
        .pick((
            ceil(double()),
            floor(double()),
            trunc(double()),
            sign(double()),
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(rows[0], (20.0, 20.0, 20.0, 1));

    let rows: Vec<(i64, i64, i64)> = db
        .select(users)
        .pick((
            greatest(users.age.as_expr().cast::<besu::Int8>(), 25_i64),
            least(users.age.as_expr().cast::<besu::Int8>(), 25_i64),
            nullif(users.age.as_expr().cast::<besu::Int8>(), 99_i64),
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(rows[0], (25, 20, 20));
}
