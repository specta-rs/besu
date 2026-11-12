#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_arguments
)]

//! Exercise the query layer end to end against a real PostgreSQL server.

mod common;

use besu::Driver;
use besu::prelude::*;
use besu_postgres::Sqlx;

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

/// A table that refers to itself: every employee has at most one manager, who is another
/// employee. It exists so a query can read the table twice, once as the employee and once as
/// the manager.
#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "employees")]
pub struct Employee {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub name: String,
    pub manager_id: Option<i64>,
}

/// A different shape for the same table, used to exercise `ALTER COLUMN`.
///
/// It lives in its own module because the derive names its table handle after the SQL table,
/// which would otherwise collide with [`users`].
#[allow(unused_imports)]
mod wide {
    use besu::prelude::*;

    #[derive(Table, Debug, Clone, PartialEq)]
    #[besu(table = "users")]
    pub struct WideUser {
        #[besu(primary_key, auto_increment)]
        pub id: i64,
        #[besu(unique)]
        pub email: String,
        pub name: String,
        pub age: i64,
        pub bio: Option<String>,
    }
}

async fn setup() -> besu::Database<Sqlx> {
    let db = common::database().await;
    common::reset(&db).await;
    db.push([users.def(), posts.def()]).await.unwrap();
    db
}

#[tokio::test]
async fn insert_and_select() {
    let db = setup().await;

    let created = db
        .insert(users)
        .value(users.insert("oscar@example.com", "Oscar", 21))
        .returning(users)
        .await
        .unwrap();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].name, "Oscar");
    assert_eq!(created[0].id, 1);
    assert_eq!(created[0].bio, None);

    let all: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(all, created);
}

#[tokio::test]
async fn generated_sql() {
    let db = besu::dry::<besu_postgres::Postgres>();

    let sql = db
        .select(users)
        .filter(users.age.gt(18))
        .filter(users.bio.is_null())
        .order_by(users.name.asc())
        .limit(10)
        .to_sql()
        .unwrap();

    // The trailing comment is the PostgreSQL driver's cache key: sqlx caches a prepared
    // statement per SQL string and PostgreSQL fixes parameter types when it is prepared.
    assert_eq!(
        sql.sql(),
        concat!(
            r#"SELECT "users"."id", "users"."email", "users"."name", "users"."age", "users"."bio" FROM "users" WHERE ("users"."age" > $1) AND ("users"."bio" IS NULL) ORDER BY "users"."name" ASC LIMIT $2"#,
            "\n/* besu:ii */"
        )
    );
    assert_eq!(sql.args().len(), 2);
}

#[tokio::test]
async fn filters_and_ordering() {
    let db = setup().await;
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
}

#[tokio::test]
async fn projection_and_aggregates() {
    let db = setup().await;
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

    let ages: Vec<f64> = db
        .select(users)
        .pick(sum(users.age.as_expr().cast::<besu::Float8>()))
        .await
        .unwrap();
    assert_eq!(ages, [50.0]);
}

#[tokio::test]
async fn update_and_delete() {
    let db = setup().await;
    db.insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .await
        .unwrap();

    let updated = db
        .update(users)
        .set(users.name, "Alicia")
        .filter(users.name.eq("Alice"))
        .returning(users)
        .await
        .unwrap();
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].name, "Alicia");

    let deleted = db
        .delete(users)
        .filter(users.name.eq("Alicia"))
        .returning(users.id)
        .await
        .unwrap();
    assert_eq!(deleted, [1_i64]);

    let remaining: Vec<User> = db.select(users).await.unwrap();
    assert!(remaining.is_empty());
}

#[tokio::test]
async fn joins() {
    let db = setup().await;
    let author = db
        .insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .returning(users)
        .await
        .unwrap()
        .remove(0);
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
}

#[tokio::test]
async fn upsert() {
    let db = setup().await;
    db.insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .await
        .unwrap();

    let rows = db
        .insert(users)
        .value(users.insert("a@example.com", "Alice", 31))
        .on_conflict(users.email)
        .do_update(users.age, 31)
        .returning(users)
        .await
        .unwrap();
    assert_eq!(rows[0].age, 31);
}

#[tokio::test]
async fn transactions() {
    let db = setup().await;

    db.transaction(async |tx| {
        tx.insert(users)
            .value(users.insert("a@example.com", "Alice", 30))
            .await?;
        Ok(())
    })
    .await
    .unwrap();

    let count: i64 = db
        .select(users)
        .pick(count_all())
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn raw_sql() {
    let db = setup().await;
    db.insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .await
        .unwrap();

    // `Fragment` rewrites the source `?` placeholders to the dialect's spelling.
    let rendered = db
        .execute(sql!("SELECT * FROM users WHERE age > ?", 18))
        .to_sql()
        .unwrap();
    assert_eq!(
        rendered.sql(),
        "SELECT * FROM users WHERE age > $1\n/* besu:i */"
    );

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
async fn null_handling() {
    let db = setup().await;

    // Not providing a nullable column lets the database default apply, which is `NULL`.
    let unset = db
        .insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .returning(users)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(unset.bio, None);

    // Providing `None` explicitly writes `NULL` instead of omitting the column.
    let explicit = db
        .insert(users)
        .value(users.insert("b@example.com", "Bob", 25).bio(None::<String>))
        .returning(users)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(explicit.bio, None);

    let with_bio = db
        .insert(users)
        .value(users.insert("c@example.com", "Carol", 40).bio("Rust"))
        .returning(users)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(with_bio.bio.as_deref(), Some("Rust"));

    let present: Vec<User> = db
        .select(users)
        .filter(users.bio.is_not_null())
        .await
        .unwrap();
    assert_eq!(present.len(), 1);
}

/// A `NULL` parameter carries no type, so the driver sends it untyped and PostgreSQL accepts
/// it for a column of any type.
#[tokio::test]
async fn null_binds_to_any_type() {
    let db = setup().await;

    db.execute(sql!("CREATE TABLE null_probe (id uuid)"))
        .await
        .unwrap();
    db.execute(sql!(
        "INSERT INTO null_probe (id) VALUES (?)",
        Option::<String>::None
    ))
    .await
    .unwrap();

    let count = db
        .execute(sql!("SELECT count(*) FROM null_probe"))
        .all::<i64>()
        .await
        .unwrap();
    assert_eq!(count, [1]);
}

#[tokio::test]
async fn bulk_update_requires_intent() {
    let db = setup().await;
    // `.all_rows()` is the only way to run an update with no `WHERE`; without it this
    // would not compile.
    db.update(users)
        .set(users.age, 40)
        .all_rows()
        .await
        .unwrap();
}

#[tokio::test]
async fn schema_push_converges() {
    let db = common::database().await;
    common::reset(&db).await;

    let first = db.push([users.def(), posts.def()]).await.unwrap();
    assert_eq!(first.changes.len(), 2);
    assert!(first.statements[0].contains("CREATE TABLE \"users\""));
    assert!(
        first
            .statements
            .iter()
            .any(|s| s.contains("GENERATED BY DEFAULT AS IDENTITY")),
        "primary keys should be identity columns: {:?}",
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
    assert_eq!(
        introspected,
        besu::Database::<Sqlx>::schema([users.def(), posts.def()])
    );

    assert_eq!(introspected.tables.len(), 2);
    let users_table = introspected.table("users").unwrap();
    assert_eq!(users_table.columns.len(), 5);
    assert!(
        users_table
            .columns
            .iter()
            .any(|c| c.name == "id" && c.auto_increment)
    );
    assert!(
        users_table
            .columns
            .iter()
            .any(|c| c.name == "email" && c.unique)
    );
    assert!(
        users_table
            .columns
            .iter()
            .any(|c| c.name == "bio" && c.nullable)
    );
    let posts_table = introspected.table("posts").unwrap();
    assert_eq!(posts_table.foreign_keys.len(), 1);
    assert_eq!(posts_table.foreign_keys[0].references_table, "users");
    assert_eq!(
        posts_table.foreign_keys[0].on_delete,
        besu::ReferenceAction::Cascade
    );
}

#[tokio::test]
async fn alter_column_changes_the_type_in_place() {
    let db = setup().await;

    let result = db.push([wide::users.def(), posts.def()]).await.unwrap();
    assert_eq!(result.changes.len(), 1);
    assert_eq!(
        result.statements,
        [r#"ALTER TABLE "users" ALTER COLUMN "age" TYPE BIGINT"#]
    );

    // The altered schema now matches, so another push is a no-op.
    let again = db.push([wide::users.def(), posts.def()]).await.unwrap();
    assert!(again.is_empty(), "got {:?}", again.changes);
}

#[tokio::test]
async fn introspection_skips_besu_migrations() {
    let db = common::database().await;
    common::reset(&db).await;
    db.execute(sql!(
        "CREATE TABLE \"_besu_migrations\" (version BIGINT PRIMARY KEY)"
    ))
    .await
    .unwrap();

    let schema = db.introspect().await.unwrap();
    assert!(schema.table("_besu_migrations").is_none());
}

/// One statement can be built with different value types; sqlx caches a prepared statement
/// per SQL string, so the driver must not let PostgreSQL reuse the types it saw first.
#[tokio::test]
async fn same_sql_text_with_different_parameter_types() {
    let db = setup().await;

    db.transaction(async |tx| {
        let text: Vec<(String,)> = tx.execute(sql!("SELECT ?::text", "abc")).all().await?;
        assert_eq!(text[0].0, "abc");

        let integer: Vec<(String,)> = tx.execute(sql!("SELECT ?::text", 1_i64)).all().await?;
        assert_eq!(integer[0].0, "1");
        Ok(())
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn json_existence_operator_with_an_escaped_question_mark() {
    let db = common::database().await;
    common::reset(&db).await;

    // `??` is an escaped question mark, so this is `$1::jsonb ? 'a'` — PostgreSQL's JSON
    // existence operator — rather than a second parameter.
    let rows: Vec<(bool,)> = db
        .execute(sql!(
            "SELECT ?::jsonb ?? 'a' AS has_key",
            r#"{"a": 1, "b": 2}"#
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(rows, [(true,)]);
}

#[tokio::test]
async fn json_path_access() {
    let db = common::database().await;
    common::reset(&db).await;

    let text: Vec<(String,)> = db
        .execute(sql!(
            "SELECT ?::jsonb #>> '{user,name}' AS name",
            r#"{"user": {"name": "Oscar"}}"#
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(text, [("Oscar".to_string(),)]);

    // The same operator comes out of the typed helper.
    let rendered = besu::dry::<besu_postgres::Postgres>();
    let sql = rendered
        .execute(sql!("SELECT ?::jsonb #>> '{user,name}'", ""))
        .to_sql()
        .unwrap();
    assert!(sql.sql().contains("#>>"), "got {}", sql.sql());
}

/// A PostgreSQL match needs no index: `to_tsvector` builds the vector for each row, so a
/// plain text column is all the set-up there is.
#[tokio::test]
async fn full_text_match() {
    let db = setup().await;
    let author = db
        .insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .returning(users)
        .await
        .unwrap()
        .remove(0);
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

    // An unrelated word matches nothing rather than everything, which is what a `LIKE`
    // left unanchored would do.
    let unrelated: Vec<Post> = db
        .select(posts)
        .filter(posts.title.matches("compiler"))
        .await
        .unwrap();
    assert!(unrelated.is_empty());

    // The query goes through the same dictionary as the document, so the stem finds the
    // inflected word: this is a word match, not a substring one.
    let stemmed: Vec<Post> = db
        .select(posts)
        .filter(matches(posts.title, "borrow"))
        .await
        .unwrap();
    assert_eq!(stemmed.len(), 1);
}

#[tokio::test]
async fn full_text_sql() {
    let db = besu::dry::<besu_postgres::Postgres>();
    let sql = db
        .select(posts)
        .filter(posts.title.matches("rust"))
        .to_sql()
        .unwrap();

    assert_eq!(
        sql.sql(),
        concat!(
            r#"SELECT "posts"."id", "posts"."title", "posts"."author_id" FROM "posts" WHERE to_tsvector('english', "posts"."title") @@ plainto_tsquery('english', $1)"#,
            "\n/* besu:t */"
        )
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
    let db = setup().await;
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
    let db = setup().await;
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
    let db = setup().await;
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

    // A scalar subquery in a projection: one value per outer row, correlated with it.
    let counts: Vec<(String, i64)> = db
        .select(users)
        .pick((
            users.name,
            scalar(
                db.select(posts)
                    .correlate(users)
                    .filter(posts.author_id.eq(users.id))
                    .pick(count_all()),
                users.id,
            ),
        ))
        .order_by(users.name.asc())
        .all()
        .await
        .unwrap();
    assert_eq!(
        counts,
        [
            ("Alice".to_string(), 2),
            ("Bob".to_string(), 0),
            ("Carol".to_string(), 0),
        ]
    );

    // The same value in a comparison, which is the other place a scalar subquery is valid.
    let prolific: Vec<User> = db
        .select(users)
        .filter(
            scalar(
                db.select(posts)
                    .correlate(users)
                    .filter(posts.author_id.eq(users.id))
                    .pick(count_all()),
                users.id,
            )
            .eq(2_i64),
        )
        .all()
        .await
        .unwrap();
    assert_eq!(names(&prolific), ["Alice"]);

    // A derived table in `FROM`: the inner query filters and projects, and the outer one
    // groups and aggregates its rows.
    let recent = derived(
        db.select(posts)
            .filter(posts.title.eq("Hello"))
            .pick(posts.author_id),
        "recent",
    );
    let author = recent.column(posts.author_id);

    let ids: Vec<i64> = db.select(&recent).all().await.unwrap();
    assert_eq!(ids, [alice.id]);

    let per_author: Vec<(i64, i64)> = db
        .select(&recent)
        .pick((author.clone(), count_all()))
        .group_by(author)
        .all()
        .await
        .unwrap();
    assert_eq!(per_author, [(alice.id, 1)]);
}

/// The exact statement PostgreSQL receives for a compound query and for each subquery form.
#[tokio::test]
async fn set_operation_and_subquery_sql() {
    let db = besu::dry::<besu_postgres::Postgres>();

    let sql = db
        .select(users)
        .pick(users.id)
        .filter(users.age.gte(18))
        .union(db.select(users).pick(users.id).filter(users.age.lt(18)))
        .order_by(users.name.asc())
        .limit(10)
        .to_sql()
        .unwrap();
    // The trailing comment is the driver's prepared-statement cache key.
    assert_eq!(
        sql.sql(),
        concat!(
            r#"SELECT "users"."id" FROM "users" WHERE ("users"."age" >= $1) UNION SELECT "users"."id" FROM "users" WHERE ("users"."age" < $2) ORDER BY "name" ASC LIMIT $3"#,
            "\n/* besu:iii */"
        )
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
        r#"SELECT "users"."id" FROM "users" WHERE EXISTS (SELECT "posts"."id", "posts"."title", "posts"."author_id" FROM "posts" WHERE ("posts"."author_id" = "users"."id"))"#
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
        r#"SELECT "users"."id" FROM "users" WHERE ("users"."id" IN (SELECT "posts"."author_id" FROM "posts"))"#
    );
}

/// An upsert can read the row it was trying to insert, and can leave the stored row alone
/// when a predicate says so. PostgreSQL is the dialect with every part of the clause, so
/// this exercises the index predicate and the update predicate against a real server.
#[tokio::test]
async fn upsert_from_the_incoming_row_and_a_predicate() {
    let db = setup().await;
    db.insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .await
        .unwrap();

    let rows: Vec<User> = db
        .insert(users)
        .value(users.insert("a@example.com", "Alice Updated", 31))
        .on_conflict(users.email)
        .do_update(users.name, excluded(users.name))
        .do_update(users.age, excluded(users.age))
        .returning(users)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "Alice Updated");
    assert_eq!(rows[0].age, 31);

    // The index predicate is inference, not a condition on the row: it still matches the
    // plain unique index on `email`.
    let rows: Vec<User> = db
        .insert(users)
        .value(users.insert("a@example.com", "Alice Again", 32))
        .on_conflict(users.email)
        .on_conflict_where(users.bio.is_null())
        .do_update(users.age, excluded(users.age))
        .returning(users)
        .await
        .unwrap();
    assert_eq!(rows[0].name, "Alice Updated");
    assert_eq!(rows[0].age, 32);

    // A false predicate skips the update, so `RETURNING` has no row to hand back and the
    // stored row keeps its name.
    let rows: Vec<User> = db
        .insert(users)
        .value(users.insert("a@example.com", "Ignored", 99))
        .on_conflict(users.email)
        .do_update(users.name, excluded(users.name))
        .do_update_filter(users.age.gt(35))
        .returning(users)
        .await
        .unwrap();
    assert!(rows.is_empty(), "a suppressed update returns no row");

    let rows: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(rows[0].name, "Alice Updated");

    // PostgreSQL is the dialect where a targetless `DO NOTHING` is the common spelling.
    db.insert(users)
        .value(users.insert("a@example.com", "Nope", 1))
        .on_conflict_all()
        .do_nothing()
        .await
        .unwrap();
    let rows: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "Alice Updated");
}

/// An empty `IN` list is a syntax error, so it renders as a constant predicate instead. If
/// the rewrite were wrong the statement would not parse at all.
#[tokio::test]
async fn empty_in_lists_execute() {
    let db = setup().await;
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

/// Every `SqlType` marker the crate ships has to survive a trip through `CAST`. PostgreSQL
/// accepts the DDL names in `CAST`, so this also pins the default `cast_type_name` down.
#[tokio::test]
async fn cast_to_every_marker_executes() {
    let db = setup().await;
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

/// The typed JSON helpers are rendered and executed, rather than asserting the raw SQL they
/// were modelled on.
#[tokio::test]
async fn typed_json_paths_render_and_execute() {
    let db = common::database().await;
    common::reset(&db).await;
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

async fn setup_notes(db: &besu::Database<Sqlx>) {
    // Pushing on its own would drop every table it is not told about, so the reference
    // tables go into the same declaration.
    db.push([users.def(), posts.def(), notes.def(), nodes.def()])
        .await
        .unwrap();
}

fn date(year: i32, month: u32, day: u32) -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn stamp(year: i32, month: u32, day: u32) -> chrono::NaiveDateTime {
    date(year, month, day).and_hms_opt(12, 0, 0).unwrap()
}

/// A constant date written into the statement. The arguments are integers chosen in the
/// test, never user input.
fn date_literal(year: i32, month: u32, day: u32) -> Expr<(NoteTable,), besu::Date> {
    Expr::raw(format!("DATE '{year:04}-{month:02}-{day:02}'"))
}

#[tokio::test]
async fn comments_are_part_of_the_statement() {
    let db = setup().await;

    let sql = db.select(users).comment("feed page").to_sql().unwrap();
    assert!(sql.sql().ends_with("/* feed page */"), "{}", sql.sql());

    let rows: Vec<User> = db.select(users).comment("feed page").all().await.unwrap();
    assert!(rows.is_empty());
}

/// PostgreSQL is the dialect that spells the null-safe comparison as a keyword, and the one
/// with the whole family of regular-expression operators.
#[tokio::test]
async fn null_safe_comparisons_and_regexes_run() {
    let db = setup().await;
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
    assert_eq!(same[0].name, "Alice");

    let matched: Vec<User> = db
        .select(users)
        .filter(users.name.regex_match("^A"))
        .all()
        .await
        .unwrap();
    assert_eq!(matched.len(), 1);

    // `~*` is the case-insensitive form, so it also finds the lowercased name.
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

    let negated_sensitive: Vec<User> = db
        .select(users)
        .filter(not_regex_match(users.name, "^A"))
        .all()
        .await
        .unwrap();
    assert_eq!(negated_sensitive.len(), 1);
    assert_eq!(negated_sensitive[0].name, "alice");
}

/// The string helpers, one call of each family, against the server that spells them the SQL
/// standard way.
#[tokio::test]
async fn string_functions_run() {
    let db = setup().await;
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
            replace(users.name, "Ali", "X"),
            left(users.name, 2),
            right(users.name, 2),
            lpad(users.name, 7, "."),
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
            "Xce".to_string(),
            "Al".to_string(),
            "ce".to_string(),
            "..Alice".to_string(),
        )
    );

    let rows: Vec<(String, String, String, i64, String, String)> = db
        .select(users)
        .pick((
            rpad(users.name, 7, "."),
            btrim(users.email),
            initcap("hello world"),
            strpos(users.name, "ice"),
            split_part("a,b,c", ",", 2),
            translate(users.name, "Al", "Ol"),
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(
        rows[0],
        (
            "Alice..".to_string(),
            "a@example.com".to_string(),
            "Hello World".to_string(),
            3,
            "b".to_string(),
            "Olice".to_string(),
        )
    );

    let rows: Vec<(String, String, String, String)> = db
        .select(users)
        .pick((
            repeat("ab", 2),
            concat_ws("-", users.name, "x"),
            md5(users.name),
            regexp_replace(users.name, "A", "O"),
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(
        rows[0],
        (
            "abab".to_string(),
            "Alice-x".to_string(),
            "64489c85dc2fe0787b85cd87214b3810".to_string(),
            "Olice".to_string(),
        )
    );

    // PostgreSQL's `sha256` returns `bytea` rather than the hexadecimal text the other
    // databases return, so it is refused rather than silently decoding as bytes.
    let error = db
        .select(users)
        .pick(sha256(users.name))
        .all()
        .await
        .unwrap_err();
    assert!(error.to_string().contains("sha256"), "{error}");
}

/// The date helpers: a truncation, an extraction, a difference and an interval shift.
#[tokio::test]
async fn date_functions_run() {
    let db = setup().await;
    setup_notes(&db).await;
    db.insert(notes)
        .value(notes.insert(
            "first".to_string(),
            date(2024, 3, 15),
            stamp(2024, 3, 15),
            true,
        ))
        .await
        .unwrap();

    let rows: Vec<(i64, i64, i64, String)> = db
        .select(notes)
        .pick((
            extract(DateField::Year, notes.created),
            extract(DateField::Month, notes.created),
            datediff(notes.created, date_literal(2024, 3, 10)),
            to_text(date_trunc(DateField::Month, notes.stamp)),
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(rows[0].0, 2024);
    assert_eq!(rows[0].1, 3);
    assert_eq!(rows[0].2, 5);
    assert_eq!(rows[0].3, "2024-03-01 00:00:00");

    let shifted: Vec<chrono::NaiveDateTime> = db
        .select(notes)
        .pick(date_add(notes.stamp, Interval::new().months(1).days(2)))
        .all()
        .await
        .unwrap();
    assert_eq!(shifted, [stamp(2024, 4, 17)]);

    let back: Vec<chrono::NaiveDateTime> = db
        .select(notes)
        .pick(date_sub(notes.stamp, Interval::new().days(15)))
        .all()
        .await
        .unwrap();
    assert_eq!(back, [stamp(2024, 2, 29)]);

    let today: Vec<chrono::NaiveDate> = db.select(notes).pick(current_date()).all().await.unwrap();
    assert_eq!(today.len(), 1);
}

/// The aggregate helpers, including the ones PostgreSQL alone has.
#[tokio::test]
async fn aggregate_functions_run() {
    let db = setup().await;
    setup_notes(&db).await;
    db.insert(notes)
        .values([
            notes.insert("a".to_string(), date(2024, 1, 1), stamp(2024, 1, 1), true),
            notes.insert("b".to_string(), date(2024, 1, 2), stamp(2024, 1, 2), false),
        ])
        .await
        .unwrap();

    let rows: Vec<(String, String, bool, bool)> = db
        .select(notes)
        .pick((
            string_agg(notes.body, ","),
            to_text(json_agg(notes.body)),
            bool_and(notes.active),
            every(notes.active),
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(
        rows,
        [("a,b".to_string(), r#"["a", "b"]"#.to_string(), false, false)]
    );

    // `bool_or` and the statistical aggregates over the whole group.
    let rows: Vec<(bool, f64, f64, String)> = db
        .select(notes)
        .pick((
            bool_or(notes.active),
            stddev(notes.id),
            variance(notes.id),
            to_text(jsonb_agg(notes.body)),
        ))
        .all()
        .await
        .unwrap();
    assert!(rows[0].0);
    assert!(rows[0].1 > 0.0);
    assert!(rows[0].2 > 0.0);
    assert_eq!(rows[0].3, r#"["a", "b"]"#);

    let objects: Vec<serde_json::Value> = db
        .select(notes)
        .pick(json_object("body", notes.body))
        .limit(1)
        .all()
        .await
        .unwrap();
    assert_eq!(objects, [serde_json::json!({ "body": "a" })]);
}

/// A window numbers the partition, and `FILTER` narrows an aggregate to some of its rows.
#[tokio::test]
async fn windows_and_filters_run() {
    let db = setup().await;
    setup_notes(&db).await;
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
            rank().over(Window::new().order_by(notes.active.asc())),
        ))
        .order_by(notes.id.asc())
        .all()
        .await
        .unwrap();
    assert_eq!(rows[0], ("a".to_string(), 1, 2));
    assert_eq!(rows[1], ("b".to_string(), 2, 1));
    assert_eq!(rows[2].1, 3);

    let counted: Vec<i64> = db
        .select(notes)
        .pick(count_all().filter(notes.active))
        .all()
        .await
        .unwrap();
    assert_eq!(counted, [2]);
}

/// An expression is defined where it is used, and a recursive one walks the tree it defines.
#[tokio::test]
async fn common_table_expressions_run() {
    let db = setup().await;
    setup_notes(&db).await;

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

/// Render any expression as text, so a value of a type the test does not decode can still be
/// checked exactly.
fn to_text<T, S: SqlType>(expr: Expr<T, S>) -> Expr<T, besu::Text> {
    expr.cast::<besu::Text>()
}

/// The numeric helpers, including the mathematical functions PostgreSQL spells directly.
#[tokio::test]
async fn numeric_functions_run() {
    let db = setup().await;
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

/// A batch runs every statement and commits them together.
#[tokio::test]
async fn batch_commits_every_statement() {
    let db = setup().await;

    let (first, second, third) = db
        .batch((
            db.insert(users)
                .value(users.insert("a@example.com", "Alice", 30)),
            db.insert(users)
                .value(users.insert("b@example.com", "Bob", 31)),
            db.update(users)
                .set(users.age, 40)
                .filter(users.email.eq("a@example.com")),
        ))
        .await
        .unwrap();

    assert_eq!(first.rows_affected(), 1);
    assert_eq!(second.rows_affected(), 1);
    assert_eq!(third.rows_affected(), 1);

    let all: Vec<User> = db.select(users).order_by(users.email.asc()).await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].age, 40);
    assert_eq!(all[1].age, 31);
}

/// A failure part-way through a batch leaves none of its writes behind.
#[tokio::test]
async fn batch_rolls_back_on_failure() {
    let db = setup().await;
    db.insert(users)
        .value(users.insert("taken@example.com", "Taken", 20))
        .await
        .unwrap();

    let result = db
        .batch((
            db.insert(users)
                .value(users.insert("a@example.com", "Alice", 30)),
            // The unique index rejects this one.
            db.insert(users)
                .value(users.insert("taken@example.com", "Duplicate", 31)),
            db.insert(users)
                .value(users.insert("c@example.com", "Carol", 32)),
        ))
        .await;

    assert!(result.is_err());

    let all: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].email, "taken@example.com");
}

// ---------------------------------------------------------------------------------------
// JSON operators and structured paths
// ---------------------------------------------------------------------------------------

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "json_docs")]
pub struct JsonDoc {
    #[besu(primary_key)]
    pub id: i64,
    /// `JSONB`, because PostgreSQL's containment and existence operators are only defined
    /// between two `jsonb` values; a plain `json` column supports `->` but not `@>`.
    #[besu(sql_type = "besu::Jsonb")]
    pub payload: serde_json::Value,
}

#[tokio::test]
async fn json_paths_are_structured_and_bound() {
    use besu::expr::{JsonPath, json_at, json_at_text, json_key, json_key_text};

    let db = common::database().await;
    common::reset(&db).await;
    db.push([json_docs.def()]).await.unwrap();

    db.insert(json_docs)
        .value(json_docs.insert(
            1,
            serde_json::json!({"a": 1, "user.name": "oscar", "b": {"c": [10, 20]}}),
        ))
        .await
        .unwrap();

    // A key containing a dot is one member, not two path segments.
    let names: Vec<String> = db
        .select(json_docs)
        .pick(json_at_text(
            json_docs.payload,
            JsonPath::root().key("user.name"),
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(names[0], "oscar");

    // `json_text` still splits on dots, so this path is two keys and finds nothing.
    let old: Vec<JsonDoc> = db
        .select(json_docs)
        .filter(is_null(json_text(json_docs.payload, "user.name")))
        .await
        .unwrap();
    assert_eq!(old.len(), 1);

    // An array index inside the path.
    let first: Vec<serde_json::Value> = db
        .select(json_docs)
        .pick(json_at(
            json_docs.payload,
            JsonPath::root().key("b").key("c").index(0),
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(first[0], serde_json::json!(10));

    // `->` and `->>` with a key that is bound rather than written into the SQL.
    let bound: Vec<String> = db
        .select(json_docs)
        .pick(json_key_text(json_docs.payload, "user.name"))
        .all()
        .await
        .unwrap();
    assert_eq!(bound[0], "oscar");

    let bound_json: Vec<serde_json::Value> = db
        .select(json_docs)
        .pick(json_key(json_docs.payload, "a"))
        .all()
        .await
        .unwrap();
    assert_eq!(bound_json[0], serde_json::json!(1));
}

#[tokio::test]
async fn json_operators_render_for_postgres() {
    use besu::expr::{
        JsonPath, contains_json, json_contained_by, json_has_all, json_has_any, json_has_key,
        json_matches, json_matches_path, json_strip,
    };

    let db = common::database().await;
    common::reset(&db).await;
    db.push([json_docs.def()]).await.unwrap();

    db.insert(json_docs)
        .value(json_docs.insert(1, serde_json::json!({"a": 1, "b": 2})))
        .await
        .unwrap();
    db.insert(json_docs)
        .value(json_docs.insert(2, serde_json::json!({"c": 3})))
        .await
        .unwrap();

    let contained: Vec<JsonDoc> = db
        .select(json_docs)
        .filter(contains_json(
            json_docs.payload,
            serde_json::json!({"a": 1}),
        ))
        .await
        .unwrap();
    assert_eq!(contained.len(), 1);

    let has_key: Vec<JsonDoc> = db
        .select(json_docs)
        .filter(json_has_key(json_docs.payload, "c"))
        .await
        .unwrap();
    assert_eq!(has_key.len(), 1);
    assert_eq!(has_key[0].id, 2);

    let any: Vec<JsonDoc> = db
        .select(json_docs)
        .filter(json_has_any(json_docs.payload, ["z", "b"]))
        .await
        .unwrap();
    assert_eq!(any.len(), 1);

    let all: Vec<JsonDoc> = db
        .select(json_docs)
        .filter(json_has_all(json_docs.payload, ["a", "b"]))
        .await
        .unwrap();
    assert_eq!(all.len(), 1);

    let matched: Vec<JsonDoc> = db
        .select(json_docs)
        .filter(json_matches_path(json_docs.payload, "$ ? (@.a == 1)"))
        .await
        .unwrap();
    assert_eq!(matched.len(), 1);

    let exact: Vec<JsonDoc> = db
        .select(json_docs)
        .filter(json_matches(json_docs.payload, "$.c == 3"))
        .await
        .unwrap();
    assert_eq!(exact.len(), 1);

    let stripped: Vec<serde_json::Value> = db
        .select(json_docs)
        .pick(json_strip(json_docs.payload, JsonPath::root().key("a")))
        .filter(json_docs.id.eq(1_i64))
        .all()
        .await
        .unwrap();
    assert_eq!(stripped[0], serde_json::json!({"b": 2}));

    let contained_by: Vec<JsonDoc> = db
        .select(json_docs)
        .filter(json_contained_by(
            json_docs.payload,
            serde_json::json!({"a": 1, "b": 2, "c": 3}),
        ))
        .await
        .unwrap();
    // Both documents are contained in the larger one, which is what `<@` means.
    assert_eq!(contained_by.len(), 2);
}

/// A self-join reads one table twice: `employees` for the employee, and the same table under
/// the alias `manager` for the person they report to. Both sides decode as `Employee`, and
/// the right-hand side is optional because the join is a `LEFT JOIN` — Alice has no manager.
#[tokio::test]
async fn self_join_with_an_alias() {
    let db = setup().await;
    // The suite shares one database, so the table this test needs is added here rather than
    // in `setup`: a `push` that does not mention every table would drop the others.
    db.push([users.def(), posts.def(), employees.def()])
        .await
        .unwrap();

    let alice = db
        .insert(employees)
        .value(employees.insert("Alice"))
        .returning(employees)
        .await
        .unwrap()
        .remove(0);
    let bob = db
        .insert(employees)
        .value(employees.insert("Bob").manager_id(alice.id))
        .returning(employees)
        .await
        .unwrap()
        .remove(0);
    db.insert(employees)
        .value(employees.insert("Carol").manager_id(alice.id))
        .await
        .unwrap();

    let manager = alias(employees, "manager");
    let rows: Vec<(Employee, Option<Employee>)> = db
        .select(employees)
        .left_join(
            &manager,
            manager.column(employees.id).eq(employees.manager_id),
        )
        .order_by(employees.id)
        .all()
        .await
        .unwrap();

    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0], (alice.clone(), None));
    assert_eq!(rows[1].0, bob);
    assert_eq!(rows[1].1.as_ref().map(|e| e.name.as_str()), Some("Alice"));
    assert_eq!(rows[2].0.name, "Carol");
    assert_eq!(rows[2].1.as_ref().map(|e| e.name.as_str()), Some("Alice"));

    // The alias is also a source in its own right: `SELECT ... FROM "employees" AS "manager"`.
    let everyone: Vec<Employee> = db.select(&manager).all().await.unwrap();
    let mut names: Vec<&str> = everyone
        .iter()
        .map(|employee| employee.name.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["Alice", "Bob", "Carol"]);
}
