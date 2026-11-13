#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_arguments
)]

use besu::Driver;
use besu::prelude::*;
use besu::types::{Bool, Int8};
use besu_sqlite::Sqlx;

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

async fn setup() -> besu::Database<Sqlx> {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.execute(sql!(
        r#"CREATE TABLE "users" (
            "id" INTEGER PRIMARY KEY AUTOINCREMENT,
            "email" TEXT NOT NULL UNIQUE,
            "name" TEXT NOT NULL,
            "age" INTEGER NOT NULL,
            "bio" TEXT
        )"#
    ))
    .await
    .unwrap();
    db.execute(sql!(
        r#"CREATE TABLE "posts" (
            "id" INTEGER PRIMARY KEY AUTOINCREMENT,
            "title" TEXT NOT NULL,
            "author_id" INTEGER NOT NULL REFERENCES "users"("id") ON DELETE CASCADE
        )"#
    ))
    .await
    .unwrap();
    db.execute(sql!(
        r#"CREATE TABLE "employees" (
            "id" INTEGER PRIMARY KEY AUTOINCREMENT,
            "name" TEXT NOT NULL,
            "manager_id" INTEGER
        )"#
    ))
    .await
    .unwrap();
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
    let db = besu::dry::<besu_sqlite::SQLite>();

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
        r#"SELECT "users"."id", "users"."email", "users"."name", "users"."age", "users"."bio" FROM "users" WHERE ("users"."age" > ?) AND ("users"."bio" IS NULL) ORDER BY "users"."name" ASC LIMIT ?"#
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

    let ages: Vec<f64> = db.select(users).pick(sum(users.age)).await.unwrap();
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
    let db = besu::Database::new(Sqlx::memory().await.unwrap());

    let first = db.push([users.def(), posts.def()]).await.unwrap();
    assert_eq!(first.changes.len(), 2);
    assert!(first.statements[0].contains("CREATE TABLE \"users\""));
    assert!(
        first.statements.iter().any(|s| s.contains("AUTOINCREMENT")),
        "auto-incrementing primary keys must be inline in SQLite: {:?}",
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

    // And the introspected schema must round-trip.
    let introspected = db.introspect().await.unwrap();
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

    // Providing `None` explicitly writes `NULL`, which is a different statement but the
    // same result for a column with no other default.
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

    let missing: Vec<User> = db.select(users).filter(users.bio.is_null()).await.unwrap();
    assert_eq!(missing.len(), 2);

    let present: Vec<User> = db
        .select(users)
        .filter(users.bio.is_not_null())
        .await
        .unwrap();
    assert_eq!(present.len(), 1);
}

#[tokio::test]
async fn expressions_and_grouping() {
    let db = setup().await;
    for (name, age) in [("Alice", 30), ("Bob", 25), ("Carol", 30)] {
        db.insert(users)
            .value(users.insert(format!("{name}@example.com"), name.to_string(), age))
            .await
            .unwrap();
    }

    // Compute the new value in the database instead of in Rust.
    db.update(users)
        .set_expr(users.age, users.age.as_expr().plus(5))
        .all_rows()
        .await
        .unwrap();
    let ages: Vec<i32> = db
        .select(users)
        .pick(users.age)
        .order_by(users.age.asc())
        .await
        .unwrap();
    assert_eq!(ages, [30, 35, 35]);

    let grouped: Vec<(i32, i64)> = db
        .select(users)
        .pick((users.age, count_all()))
        .group_by(users.age)
        .having(count_all().gt(1))
        .order_by(users.age.asc())
        .await
        .unwrap();
    assert_eq!(grouped, [(35, 2)]);

    let distinct: Vec<i32> = db
        .select(users)
        .pick(users.age)
        .distinct()
        .order_by(users.age.asc())
        .await
        .unwrap();
    assert_eq!(distinct, [30, 35]);

    let page: Vec<String> = db
        .select(users)
        .pick(users.name)
        .order_by(users.name.asc())
        .limit(1)
        .offset(1)
        .await
        .unwrap();
    assert_eq!(page, ["Bob"]);

    let between: Vec<String> = db
        .select(users)
        .filter(users.age.between(28, 32))
        .pick(users.name)
        .await
        .unwrap();
    assert_eq!(between.len(), 1);

    let casted: Vec<String> = db
        .select(users)
        .pick(users.age.as_expr().cast::<besu::Text>())
        .order_by(users.age.asc())
        .limit(1)
        .await
        .unwrap();
    assert_eq!(casted, ["30"]);
}

#[tokio::test]
async fn upsert_roundtrip_and_conflict_target_tuple() {
    let db = setup().await;
    db.insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .await
        .unwrap();

    // A composite conflict target is accepted as a tuple.
    db.insert(users)
        .value(users.insert("a@example.com", "Alice", 31))
        .on_conflict(users.email)
        .do_update(users.age, 31)
        .await
        .unwrap();

    let all: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].age, 31);
}

#[tokio::test]
async fn left_join_projection_widens_to_option() {
    let db = setup().await;
    // A user with no posts, to make the outer side actually be absent.
    db.insert(users)
        .value(users.insert("lonely@example.com", "Lonely", 30))
        .await
        .unwrap();
    let alice = db
        .insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .returning(users)
        .await
        .unwrap()
        .remove(0);
    db.insert(posts)
        .value(posts.insert("Hello", alice.id))
        .await
        .unwrap();

    // `posts.title` is `NOT NULL`, but on the outer side of a left join it can be absent, so
    // the projection widens to `Option<String>` without being asked.
    let rows: Vec<(String, Option<String>)> = db
        .select(users)
        .left_join(posts, eq(users.id, posts.author_id))
        .pick((users.name, posts.title))
        .order_by(users.name.asc())
        .await
        .unwrap();

    assert_eq!(
        rows,
        [
            ("Alice".to_string(), Some("Hello".to_string())),
            ("Lonely".to_string(), None),
        ]
    );
}

#[tokio::test]
async fn raw_sql_accepts_runtime_values() {
    let db = setup().await;
    db.insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .await
        .unwrap();

    // The compile-time arity check must not require its arguments to be constants.
    let name = String::from("Alice");
    let minimum_age = 18;
    let rows: Vec<User> = db
        .execute(sql!(
            "SELECT * FROM users WHERE name = ? AND age > ?",
            name,
            minimum_age
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[tokio::test]
async fn pick_whole_tables_from_a_left_join() {
    let db = setup().await;
    let alice = db
        .insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .returning(users)
        .await
        .unwrap()
        .remove(0);
    db.insert(users)
        .value(users.insert("lonely@example.com", "Lonely", 30))
        .await
        .unwrap();
    db.insert(posts)
        .value(posts.insert("Hello", alice.id))
        .await
        .unwrap();

    // Selecting a whole table from the outer side widens to `Option` for the same reason a
    // single column does.
    let rows: Vec<(User, Option<Post>)> = db
        .select(users)
        .left_join(posts, eq(users.id, posts.author_id))
        .pick((users, posts))
        .order_by(users.name.asc())
        .await
        .unwrap();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0.name, "Alice");
    assert_eq!(rows[0].1.as_ref().unwrap().title, "Hello");
    assert_eq!(rows[1].0.name, "Lonely");
    assert!(rows[1].1.is_none());
}

#[tokio::test]
async fn raw_query_first_returns_the_first_row() {
    let db = setup().await;
    for (name, age) in [("Alice", 30), ("Bob", 20)] {
        db.insert(users)
            .value(users.insert(format!("{name}@example.com"), name, age))
            .await
            .unwrap();
    }

    let first = db
        .execute(sql!("SELECT name FROM users ORDER BY name ASC"))
        .first::<(String,)>()
        .await
        .unwrap()
        .map(|(name,)| name);
    assert_eq!(first.as_deref(), Some("Alice"));
}

#[tokio::test]
async fn case_expressions() {
    let db = setup().await;
    for (name, age) in [("Alice", 30), ("Bob", 12)] {
        db.insert(users)
            .value(users.insert(format!("{name}@example.com"), name, age))
            .await
            .unwrap();
    }

    let rows: Vec<(String, String)> = db
        .select(users)
        .pick((
            users.name,
            // The result type is written out because a bare `"adult"` could equally be a
            // `TEXT`, `VARCHAR(n)` or `CHAR(n)` value; the branches are checked against it.
            case::<_, besu::Text>()
                .when(users.age.gte(18), "adult")
                .otherwise("minor"),
        ))
        .order_by(users.name.asc())
        .await
        .unwrap();

    assert_eq!(
        rows,
        [
            ("Alice".to_string(), "adult".to_string()),
            ("Bob".to_string(), "minor".to_string()),
        ]
    );
}

#[tokio::test]
async fn update_from_a_model() {
    let db = setup().await;
    let original = db
        .insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .returning(users)
        .await
        .unwrap()
        .remove(0);

    let changed = User {
        name: "Alicia".into(),
        age: 31,
        bio: Some("Rust".into()),
        ..original.clone()
    };

    db.update(users)
        .set_model(&changed)
        .filter(users.id.eq(original.id))
        .await
        .unwrap();

    let stored: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].name, "Alicia");
    assert_eq!(stored[0].age, 31);
    assert_eq!(stored[0].bio.as_deref(), Some("Rust"));
    // The primary key is never rewritten by `set_model`.
    assert_eq!(stored[0].id, original.id);
}

#[tokio::test]
async fn chunked_insert_splits_across_statements() {
    let db = setup().await;

    // 500 rows with 3 bound columns each would be 1,500 parameters in one statement. Split
    // into batches of 100 so each stays small, which is what a bulk import has to do.
    let rows: Vec<_> = (0..500u32)
        .map(|i| users.insert(format!("user{i}@example.com"), format!("User {i}"), 20))
        .collect();

    let outputs = db.insert(users).values_chunked(rows, 100).await.unwrap();
    assert_eq!(outputs.len(), 5);

    let count: i64 = db.select(users).pick(count_all()).await.unwrap().remove(0);
    assert_eq!(count, 500);
}

#[tokio::test]
async fn too_many_parameters_is_a_clear_error() {
    let db = setup().await;

    // SQLite accepts 32,766 parameters; 40,000 in one `IN` list is over the limit, and the
    // error should say so rather than surfacing a driver message.
    let values: Vec<i64> = (0..40_000).collect();
    let error = db
        .select(users)
        .filter(users.id.in_list(values))
        .await
        .unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("parameters"),
        "expected a parameter-limit error, got: {message}"
    );
}

#[tokio::test]
async fn streaming_a_result_set() {
    let db = setup().await;
    let rows: Vec<_> = (0..200u32)
        .map(|i| users.insert(format!("s{i}@example.com"), format!("S {i}"), 30))
        .collect();
    db.insert(users).values(rows).await.unwrap();

    // A cursor borrows the SQL text, so the statement is rendered first and kept alive.
    let sql = db
        .select(users)
        .filter(users.age.eq(30))
        .order_by(users.id.asc())
        .to_sql()
        .unwrap();
    let mut rows = db.stream::<User>(&sql).await.unwrap();

    let mut count = 0;
    while let Some(user) = rows.next().await {
        let user = user.unwrap();
        assert_eq!(user.age, 30);
        count += 1;
    }
    assert_eq!(count, 200);

    // And the stream is a real `Stream`, so `collect` is available for small result sets.
    let sql = db.select(users).limit(5).to_sql().unwrap();
    let collected = db
        .stream::<User>(&sql)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(collected.len(), 5);
}

#[tokio::test]
async fn streaming_inside_a_transaction() {
    let db = setup().await;
    db.insert(users)
        .values([
            users.insert("a@example.com", "Alice", 30),
            users.insert("b@example.com", "Bob", 20),
        ])
        .await
        .unwrap();

    let mut tx = db.begin().await.unwrap();
    let sql = tx.select(users).order_by(users.id.asc()).to_sql().unwrap();
    let mut rows = tx.stream::<User>(&sql).await.unwrap();
    let mut names = Vec::new();
    while let Some(user) = rows.next().await {
        names.push(user.unwrap().name);
    }
    drop(rows);
    tx.commit().await.unwrap();

    assert_eq!(names, ["Alice", "Bob"]);
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
    for (name, age) in [("Alice", 30), ("Bob", 17), ("Carol", 25), ("Dave", 40)] {
        db.insert(users)
            .value(users.insert(format!("{name}@example.com"), name.to_string(), age))
            .await
            .unwrap();
    }

    // Two disjoint filters: the union is every row, the intersection is empty and the
    // difference is the first filter.
    let everyone: Vec<User> = db
        .select(users)
        .filter(users.age.gte(18))
        .union(db.select(users).filter(users.age.lt(18)))
        .order_by(users.name.asc())
        .all()
        .await
        .unwrap();
    assert_eq!(names(&everyone), ["Alice", "Bob", "Carol", "Dave"]);

    let adults: Vec<User> = db
        .select(users)
        .filter(users.age.gte(18))
        .intersect(db.select(users).filter(users.age.lt(18)))
        .all()
        .await
        .unwrap();
    assert!(adults.is_empty());

    // Overlapping filters: `union` collapses the duplicate, `union_all` keeps both copies.
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

/// SQLite has neither `INTERSECT ALL` nor `EXCEPT ALL`, so those queries are rejected while
/// the statement is built rather than sent to a server that cannot parse them.
#[tokio::test]
async fn the_all_operators_are_unsupported_on_sqlite() {
    let db = setup().await;

    let error = db
        .select(users)
        .intersect_all(db.select(users))
        .all()
        .await
        .unwrap_err();
    assert!(error.to_string().contains("INTERSECT ALL"), "{error}");

    let error = db
        .select(users)
        .except_all(db.select(users))
        .all()
        .await
        .unwrap_err();
    assert!(error.to_string().contains("EXCEPT ALL"), "{error}");
}

/// `exists` and `in_query` are subqueries of the enclosing statement: their parameters are
/// part of the same numbering and their rows are not decoded separately.
#[tokio::test]
async fn subqueries() {
    let db = setup().await;
    for (name, age) in [("Alice", 30), ("Bob", 17), ("Carol", 25)] {
        db.insert(users)
            .value(users.insert(format!("{name}@example.com"), name.to_string(), age))
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

    // A correlated `EXISTS`: the subquery reads `users.id` from the enclosing query.
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

    // The same correlation with a condition inside the subquery, which proves the inner
    // filter is part of the subquery rather than the outer one.
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
    let goodbye: Vec<User> = db
        .select(users)
        .filter(exists(
            db.select(posts)
                .correlate(users)
                .filter(posts.author_id.eq(users.id))
                .filter(posts.title.eq("Goodbye")),
        ))
        .all()
        .await
        .unwrap();
    assert!(goodbye.is_empty());

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

    // The subquery carries its own filter, so only Alice's "World" post authors survive.
    let world: Vec<User> = db
        .select(users)
        .filter(
            users.id.in_query(
                db.select(posts)
                    .filter(posts.title.eq("World"))
                    .pick(posts.author_id),
            ),
        )
        .all()
        .await
        .unwrap();
    assert_eq!(names(&world), ["Alice"]);

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

/// The exact statement SQLite receives for a compound query and for each subquery form.
#[tokio::test]
async fn set_operation_and_subquery_sql() {
    let db = besu::dry::<besu_sqlite::SQLite>();

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
        r#"SELECT "users"."id" FROM "users" WHERE ("users"."age" >= ?) UNION SELECT "users"."id" FROM "users" WHERE ("users"."age" < ?) ORDER BY "name" ASC LIMIT ?"#
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

/// A compound query is a statement, so SQLite accepts it in `EXISTS`, `IN`, a scalar position
/// and a derived table. Each of these runs the same compound, whose branches keep their own
/// filters and whose `LIMIT` belongs to the result.
#[tokio::test]
async fn a_set_operation_is_a_subquery() {
    let db = setup().await;
    for (name, age) in [("Alice", 30), ("Bob", 17), ("Carol", 25)] {
        db.insert(users)
            .value(users.insert(format!("{name}@example.com"), name.to_string(), age))
            .await
            .unwrap();
    }

    // `EXISTS` over a compound: the compound selects every user, so every outer row matches.
    let everyone: Vec<User> = db
        .select(users)
        .filter(exists(
            db.select(users)
                .pick(users.id)
                .filter(users.age.gte(18))
                .union(db.select(users).pick(users.id).filter(users.age.lt(18))),
        ))
        .order_by(users.name.asc())
        .all()
        .await
        .unwrap();
    assert_eq!(names(&everyone), ["Alice", "Bob", "Carol"]);

    // `IN` over the same compound. The branches are `age >= 18` and `age < 18`, so together
    // they cover every row exactly once.
    let ids: Vec<User> = db
        .select(users)
        .filter(
            users.id.in_query(
                db.select(users)
                    .pick(users.id)
                    .filter(users.age.gte(18))
                    .union(db.select(users).pick(users.id).filter(users.age.lt(18))),
            ),
        )
        .order_by(users.name.asc())
        .all()
        .await
        .unwrap();
    assert_eq!(names(&ids), ["Alice", "Bob", "Carol"]);

    // A scalar position needs one row and one column; the compound's `LIMIT` is applied to
    // the whole result.
    let min_adult: i64 = db
        .select(users)
        .pick(scalar(
            db.select(users)
                .pick(users.id)
                .filter(users.age.gte(18))
                .union(db.select(users).pick(users.id).filter(users.age.lt(18)))
                .order_by(users.id.asc())
                .limit(1),
            users.id,
        ))
        .filter(users.name.eq("Carol"))
        .first()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(min_adult, 1, "Alice was inserted first");

    // A derived table reads the compound through its alias, and the default projection is
    // the first branch's projection.
    let everyone = derived(
        db.select(users)
            .pick(users.id)
            .filter(users.age.gte(18))
            .union(db.select(users).pick(users.id).filter(users.age.lt(18))),
        "everyone",
    );
    let mut ids: Vec<i64> = db.select(&everyone).all().await.unwrap();
    ids.sort_unstable();
    assert_eq!(ids, [1, 2, 3]);

    // The alias is a table, so the outer query can aggregate it.
    let id = everyone.column(users.id);
    let count: i64 = db
        .select(&everyone)
        .pick(count_all())
        .filter(id.gt(1_i64))
        .first()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(count, 2);
}

/// A projected expression is named at the call site, and its columns are read back by name
/// with a type witness. A compound gets the same constructor.
#[tokio::test]
async fn a_projected_common_table_expression_is_read_by_name() {
    let db = setup().await;
    db.insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .await
        .unwrap();
    db.insert(users)
        .value(users.insert("b@example.com", "Bob", 17))
        .await
        .unwrap();
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

    let totals = db
        .select(posts)
        .pick((posts.author_id, count_all()))
        .cte_named("totals", (("author_id", posts.author_id), ("total", Int8)));
    let author = totals.column_named("author_id", posts.author_id);
    let total = totals.column_named("total", Int8);

    let rows: Vec<(i64, i64)> = db
        .from(&totals)
        .pick((author.clone(), total.clone()))
        .filter(total.gt(0_i64))
        .all()
        .await
        .unwrap();
    assert_eq!(rows, [(alice.id, 2)]);

    // A compound is a valid `WITH` body, and its first branch names the output columns.
    let everyone = db
        .select(users)
        .pick(users.id)
        .union(db.select(posts).pick(posts.author_id))
        .cte_named("everyone", (("id", users.id),));
    let mut ids: Vec<i64> = db
        .from(&everyone)
        .pick(everyone.column_named("id", users.id))
        .all()
        .await
        .unwrap();
    ids.sort_unstable();
    assert_eq!(ids, [1, 2]);

    // The whole-row form still works and still reads columns from the schema.
    let adults = db.select(users).filter(users.age.gte(18)).cte("adults");
    let rows: Vec<User> = db
        .from(&adults)
        .filter(adults.column(users.age).lt(65))
        .all()
        .await
        .unwrap();
    assert_eq!(names(&rows), ["Alice"]);
}

/// The dynamic handle takes erased clauses, so a query can be assembled where the row-set
/// type-state is in the way. `count` wraps either shape in a derived table.
#[tokio::test]
async fn a_dynamic_query_and_count_run() {
    let db = setup().await;
    for (name, age) in [("Alice", 30), ("Bob", 17), ("Carol", 25)] {
        db.insert(users)
            .value(users.insert(format!("{name}@example.com"), name.to_string(), age))
            .await
            .unwrap();
    }

    let filter: Expr<(UserTable,), Bool> = users.age.gte(18);
    let order: OrderClause = users.name.asc().into_order();
    let rows: Vec<User> = db
        .select(users)
        .dynamic()
        .filter(filter.into_kind())
        .order_by(order)
        .limit(1)
        .all()
        .await
        .unwrap();
    assert_eq!(names(&rows), ["Alice"]);

    let adults: i64 = db
        .select(users)
        .filter(users.age.gte(18))
        .count()
        .first()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(adults, 2);

    let total: i64 = db
        .select(users)
        .pick(users.id)
        .filter(users.age.gte(18))
        .union(db.select(users).pick(users.id).filter(users.age.lt(18)))
        .count()
        .first()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(total, 3);

    // `LIMIT` is part of the statement being counted, so it caps the count.
    let capped: i64 = db
        .select(users)
        .pick(users.id)
        .filter(users.age.gte(18))
        .union(db.select(users).pick(users.id).filter(users.age.lt(18)))
        .limit(2)
        .count()
        .first()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(capped, 2);
}

/// An upsert can read the row it was trying to insert, and can leave the stored row alone
/// when a predicate says so. Both are pure SQLite syntax (`excluded` is the standard name
/// SQLite shares with PostgreSQL), so nothing here needs a dialect branch.
#[tokio::test]
async fn upsert_from_the_incoming_row_and_a_predicate() {
    let db = setup().await;
    db.insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .await
        .unwrap();

    db.insert(users)
        .value(users.insert("a@example.com", "Alice Updated", 31))
        .on_conflict(users.email)
        .do_update(users.name, excluded(users.name))
        .do_update(users.age, excluded(users.age))
        .await
        .unwrap();

    let rows: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "Alice Updated");
    assert_eq!(rows[0].age, 31);

    // The update's predicate is evaluated against the stored row, so a false one suppresses
    // the update and keeps the row as it was.
    db.insert(users)
        .value(users.insert("a@example.com", "Ignored", 99))
        .on_conflict(users.email)
        .do_update(users.name, excluded(users.name))
        .do_update_filter(users.age.gt(35))
        .await
        .unwrap();

    let rows: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(rows[0].name, "Alice Updated");

    // SQLite allows a targetless `DO NOTHING`, so the other half of the clause works too.
    db.insert(users)
        .value(users.insert("a@example.com", "Nope", 1))
        .on_conflict_all()
        .do_nothing()
        .await
        .unwrap();

    let rows: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "Alice Updated");

    // Unlike PostgreSQL, SQLite also allows `DO UPDATE` with no target: the clause applies
    // to any unique constraint. The rendered statement is checked before it runs.
    let query = db
        .insert(users)
        .value(users.insert("a@example.com", "Any Key", 50))
        .on_conflict_all()
        .do_update(users.name, excluded(users.name));
    let sql = query.to_sql().unwrap();
    assert_eq!(
        sql.sql(),
        r#"INSERT INTO "users" ("email", "name", "age") VALUES (?, ?, ?) ON CONFLICT DO UPDATE SET "name" = "excluded"."name""#
    );
    query.await.unwrap();

    let rows: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(rows[0].name, "Any Key");
    assert_eq!(rows[0].age, 31);
}

/// SQLite has no catalogue entry for a partial index's predicate, so the only way to know
/// the rendered `ON CONFLICT (col) WHERE ...` matches one is to create the index and run it.
#[tokio::test]
async fn a_conflict_target_matches_a_partial_index() {
    let db = setup().await;
    db.execute(sql!(
        r#"CREATE UNIQUE INDEX "users_email_active" ON "users" ("email") WHERE "age" > 0"#
    ))
    .await
    .unwrap();

    db.insert(users)
        .value(users.insert("partial@example.com", "First", 30))
        .await
        .unwrap();

    let query = db
        .insert(users)
        .value(users.insert("partial@example.com", "Second", 40))
        .on_conflict(users.email)
        .on_conflict_where(users.age.gt(0))
        .do_update(users.name, excluded(users.name));
    let sql = query.to_sql().unwrap();
    assert!(
        sql.sql()
            .contains(r#"ON CONFLICT ("email") WHERE ("users"."age" > ?)"#),
        "got: {}",
        sql.sql()
    );
    query.await.unwrap();

    let rows: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "Second");
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

    let none: Vec<User> = db
        .select(users)
        .filter(in_list(users.name, Vec::<&str>::new()))
        .await
        .unwrap();
    assert!(none.is_empty());
}

/// Every `SqlType` marker the crate ships has to survive a trip through `CAST`. SQLite
/// accepts the DDL names, so this also pins the default `cast_type_name` down.
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

// ---------------------------------------------------------------------------------------
// Comments, operators, functions and common table expressions
// ---------------------------------------------------------------------------------------

/// A table with a date and a boolean, so the date and aggregate functions have something to
/// work on that `users` and `posts` do not provide.
#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "notes")]
pub struct Note {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub body: String,
    pub created: chrono::NaiveDate,
    pub active: bool,
}

/// A self-referencing table, so a recursive expression has something to walk.
#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "nodes")]
pub struct Node {
    // Deliberately not marked as the primary key: the derive leaves a primary key out of
    // the insert shape, and this test supplies the ids so the tree is deterministic.
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
}

async fn setup_notes(db: &besu::Database<Sqlx>) {
    db.execute(sql!(
        r#"CREATE TABLE "notes" (
            "id" INTEGER PRIMARY KEY AUTOINCREMENT,
            "body" TEXT NOT NULL,
            "created" DATE NOT NULL,
            "active" BOOLEAN NOT NULL
        )"#
    ))
    .await
    .unwrap();
    db.execute(sql!(
        r#"CREATE TABLE "nodes" (
            "id" INTEGER PRIMARY KEY,
            "parent_id" INTEGER,
            "name" TEXT NOT NULL
        )"#
    ))
    .await
    .unwrap();
}

fn date(year: i32, month: u32, day: u32) -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

/// A date written into the statement, for the side of a comparison that is a constant
/// rather than a column. The arguments are integers chosen in the test, never user input.
fn date_literal(year: i32, month: u32, day: u32) -> Expr<(NoteTable,), besu::Date> {
    // SQLite has no typed date literal; a text value in the ISO form is what its date
    // functions parse.
    Expr::raw(format!("'{year:04}-{month:02}-{day:02}'"))
}

/// Every builder takes a comment, it renders last, and the statement still runs.
#[tokio::test]
async fn comments_are_part_of_the_statement() {
    let db = setup().await;

    let sql = db.select(users).comment("feed page").to_sql().unwrap();
    assert!(sql.sql().ends_with("/* feed page */"), "{}", sql.sql());

    let rows: Vec<User> = db.select(users).comment("feed page").all().await.unwrap();
    assert!(rows.is_empty());

    db.insert(users)
        .value(users.insert("oscar@example.com", "Oscar", 21))
        .comment("signup")
        .await
        .unwrap();

    let sql = db
        .update(users)
        .set(users.name, "Oscar")
        .filter(users.id.eq(1_i64))
        .comment("rename")
        .to_sql()
        .unwrap();
    assert!(sql.sql().ends_with("/* rename */"), "{}", sql.sql());
}

/// `IS DISTINCT FROM` compares `NULL` as a value, which `<>` does not.
#[tokio::test]
async fn null_safe_comparisons_run() {
    let db = setup().await;
    db.insert(users)
        .value(users.insert("a@example.com", "A", 20))
        .await
        .unwrap();
    db.insert(users)
        .value(
            users
                .insert("b@example.com", "B", 30)
                .bio("hello".to_string()),
        )
        .await
        .unwrap();

    // `bio = 'hello'` is unknown for the row whose bio is NULL, so `<>` misses it; the
    // null-safe form is true there.
    let differing: Vec<User> = db
        .select(users)
        .filter(users.bio.is_distinct_from("hello"))
        .all()
        .await
        .unwrap();
    assert_eq!(names(&differing), ["A"]);

    let same: Vec<User> = db
        .select(users)
        .filter(users.bio.is_not_distinct_from(Option::<String>::None))
        .all()
        .await
        .unwrap();
    assert_eq!(names(&same), ["A"]);
}

/// SQLite has no regular-expression matching without an extension, so the statement is
/// rejected while it is built instead of being sent to fail there.
#[tokio::test]
async fn regular_expressions_are_unsupported_on_sqlite() {
    let db = setup().await;
    let error = db
        .select(users)
        .filter(users.name.regex_match("^A"))
        .all()
        .await
        .unwrap_err();
    assert!(error.to_string().contains("regular-expression"), "{error}");
}

/// The string helpers are spelled the SQLite way: `length` for `char_length`, `trim` for
/// `btrim` and `instr` for `strpos`.
#[tokio::test]
async fn string_functions_run() {
    let db = setup().await;
    db.insert(users)
        .value(users.insert("a@example.com", "  Oscar  ", 20))
        .await
        .unwrap();

    let rows: Vec<(String, String, i64, i64, String, i64)> = db
        .select(users)
        .pick((
            upper(users.name),
            btrim(users.name),
            char_length(users.name),
            strpos(users.name, "Oscar"),
            replace(users.name, "Oscar", "Sam"),
            length(users.email),
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(
        rows[0],
        (
            "  OSCAR  ".to_string(),
            "Oscar".to_string(),
            9,
            3,
            "  Sam  ".to_string(),
            13
        )
    );

    let joined: Vec<String> = db
        .select(users)
        .pick(concat(users.name, "!"))
        .all()
        .await
        .unwrap();
    assert_eq!(joined, ["  Oscar  !"]);
}

/// The date helpers on SQLite: `current_date`, `EXTRACT` through `strftime`, a difference of
/// two dates and a shift by an interval.
#[tokio::test]
async fn date_functions_run() {
    let db = setup().await;
    setup_notes(&db).await;
    db.insert(notes)
        .value(notes.insert("first".to_string(), date(2024, 3, 15), true))
        .await
        .unwrap();

    let rows: Vec<(i64, i64, i64, i64)> = db
        .select(notes)
        .pick((
            extract(DateField::Year, notes.created),
            extract(DateField::Month, notes.created),
            datediff(notes.created, date_literal(2024, 3, 10)),
            extract(DateField::Quarter, notes.created),
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(rows[0], (2024, 3, 5, 1));

    let shifted: Vec<chrono::NaiveDate> = db
        .select(notes)
        .pick(date_add(notes.created, Interval::new().days(1)))
        .all()
        .await
        .unwrap();
    assert_eq!(shifted, [date(2024, 3, 16)]);

    let today: Vec<chrono::NaiveDate> = db.select(notes).pick(current_date()).all().await.unwrap();
    assert_eq!(today.len(), 1);
}

/// `string_agg` is `group_concat` here, `bool_and` and `bool_or` are `min` and `max`, and
/// `json_agg` is `json_group_array`. The PostgreSQL-only aggregates are rejected.
#[tokio::test]
async fn aggregate_functions_run() {
    let db = setup().await;
    setup_notes(&db).await;
    db.insert(notes)
        .values([
            notes.insert("a".to_string(), date(2024, 1, 1), true),
            notes.insert("b".to_string(), date(2024, 1, 2), false),
        ])
        .await
        .unwrap();

    let joined: Vec<String> = db
        .select(notes)
        .pick(string_agg(notes.body, ","))
        .all()
        .await
        .unwrap();
    assert_eq!(joined, ["a,b"]);

    let rows: Vec<(bool, bool, serde_json::Value)> = db
        .select(notes)
        .pick((
            bool_and(notes.active),
            bool_or(notes.active),
            json_agg(notes.body),
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(rows, [(false, true, serde_json::json!(["a", "b"]))]);

    let error = db
        .select(notes)
        .pick(stddev(notes.id))
        .all()
        .await
        .unwrap_err();
    assert!(error.to_string().contains("stddev"), "{error}");
}

/// A window function numbers the rows of a partition, and `FILTER` narrows an aggregate to
/// some of them.
#[tokio::test]
async fn windows_and_filters_run() {
    let db = setup().await;
    setup_notes(&db).await;
    db.insert(notes)
        .values([
            notes.insert("a".to_string(), date(2024, 1, 1), true),
            notes.insert("b".to_string(), date(2024, 1, 2), false),
            notes.insert("c".to_string(), date(2024, 1, 3), true),
        ])
        .await
        .unwrap();

    let rows: Vec<(String, i64)> = db
        .select(notes)
        .pick((
            notes.body,
            row_number().over(Window::new().order_by(notes.id.asc())),
        ))
        .order_by(notes.id.asc())
        .all()
        .await
        .unwrap();
    assert_eq!(
        rows,
        [
            ("a".to_string(), 1),
            ("b".to_string(), 2),
            ("c".to_string(), 3)
        ]
    );

    let counted: Vec<i64> = db
        .select(notes)
        .pick(count_all().filter(notes.active))
        .all()
        .await
        .unwrap();
    assert_eq!(counted, [2]);

    let jumped: Vec<(String, i64)> = db
        .select(notes)
        .pick((
            notes.body,
            lag(notes.id).over(Window::new().order_by(notes.id.asc())),
        ))
        .order_by(notes.id.asc())
        .all()
        .await
        .unwrap();
    // The first row has no predecessor, so it is `NULL`; `lag` is only used here for its
    // place in the window, so the row is read as the id it lagged.
    assert_eq!(jumped.len(), 3);
}

/// A non-recursive expression is defined where it is used and read back, and a recursive one
/// walks the tree it defines.
#[tokio::test]
async fn common_table_expressions_run() {
    let db = setup().await;
    setup_notes(&db).await;

    let adults = db.select(users).filter(users.age.gte(18)).cte("adults");
    db.insert(users)
        .values([
            users.insert("a@example.com", "A", 20),
            users.insert("b@example.com", "B", 30),
            users.insert("c@example.com", "C", 10),
        ])
        .await
        .unwrap();

    let rows: Vec<User> = db
        .from(&adults)
        .filter(adults.column(users.age).lt(25))
        .all()
        .await
        .unwrap();
    assert_eq!(names(&rows), ["A"]);

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

/// The numeric helpers, including the mathematical functions SQLite only has when its
/// amalgamation is built with `SQLITE_ENABLE_MATH_FUNCTIONS` — which is how `sqlx` builds it.
#[tokio::test]
async fn numeric_functions_run() {
    let db = setup().await;
    db.insert(users)
        .value(users.insert("a@example.com", "A", 20))
        .await
        .unwrap();

    let rows: Vec<(f64, f64, f64, f64, f64)> = db
        .select(users)
        .pick((
            abs(users.age.as_expr().cast::<besu::Float8>()),
            round(users.age.as_expr().cast::<besu::Float8>()),
            round_to(users.age.as_expr().cast::<besu::Float8>(), 1),
            round_to(users.age.as_expr().cast::<besu::Float8>(), 0),
            abs(users.age.as_expr().cast::<besu::Float8>()),
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(rows[0].0, 20.0);
    assert_eq!(rows[0].1, 20.0);
    assert_eq!(rows[0].2, 20.0);
    assert_eq!(rows[0].3, 20.0);
    assert_eq!(rows[0].4, 20.0);

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
    assert_eq!(rows[0].0, 25);
    assert_eq!(rows[0].1, 20);
    // `nullif` returns the first argument unless the two are equal.
    assert_eq!(rows[0].2, 20);

    // SQLite's mathematical functions need `SQLITE_ENABLE_MATH_FUNCTIONS`, which the
    // amalgamation `sqlx` builds does not set, so the ones it spells through that library
    // record an error rather than being sent to fail there.
    let error = db
        .select(users)
        .pick(sqrt(users.age.as_expr().cast::<besu::Float8>()))
        .all()
        .await
        .unwrap_err();
    assert!(error.to_string().contains("sqrt"), "{error}");
}

/// A self-join reads one table twice: `employees` for the employee, and the same table under
/// the alias `manager` for the person they report to. Both sides decode as `Employee`, and
/// the right-hand side is optional because the join is a `LEFT JOIN` — Alice has no manager.
#[tokio::test]
async fn self_join_with_an_alias() {
    let db = setup().await;

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

/// Two aliases of one table in one statement: `report` is the employee and `manager` is the
/// same table under a different tag. Each side of the join condition is qualified by its own
/// alias, and both rows decode as `Employee`.
#[tokio::test]
async fn self_join_with_two_aliases() {
    let db = setup().await;

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
        .value(employees.insert("Carol").manager_id(bob.id))
        .await
        .unwrap();

    let report = alias(employees, "report");
    let manager = alias_as::<_, 1>(employees, "manager");
    let mut rows: Vec<(Employee, Employee)> = db
        .select(report)
        .inner_join(
            &manager,
            manager
                .column(employees.id)
                .eq(report.column(employees.manager_id)),
        )
        .all()
        .await
        .unwrap();

    rows.sort_by(|left, right| left.0.name.cmp(&right.0.name));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0.name, "Bob");
    assert_eq!(rows[0].1.name, "Alice");
    assert_eq!(rows[1].0.name, "Carol");
    assert_eq!(rows[1].1.name, "Bob");
}
