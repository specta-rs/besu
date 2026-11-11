#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_arguments
)]

//! Exercise the query layer end to end against a real PostgreSQL server.

mod common;

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
