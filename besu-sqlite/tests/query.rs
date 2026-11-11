#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_arguments
)]

use besu::Driver;
use besu::prelude::*;
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
