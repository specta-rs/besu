use besu::prelude::*;
use besu_postgres::Sqlx;

/// The server the test suite runs against. It is a real PostgreSQL instance, so the whole
/// suite takes a lock and resets the tables around each test.
const URL: &str = "postgres://besu:besu@localhost:55432/besu";

static DB_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

fn lock() -> std::sync::MutexGuard<'static, ()> {
    DB_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

async fn database() -> besu::Database<Sqlx> {
    besu::Database::new(Sqlx::new(URL).await.unwrap())
}

async fn drop_tables(db: &besu::Database<Sqlx>) {
    db.execute(sql!(
        "DROP TABLE IF EXISTS posts, users, null_probe CASCADE"
    ))
    .await
    .unwrap();
}

async fn setup() -> (std::sync::MutexGuard<'static, ()>, besu::Database<Sqlx>) {
    let guard = lock();
    let db = database().await;
    drop_tables(&db).await;
    db.push([users.def(), posts.def()]).await.unwrap();
    (guard, db)
}

#[tokio::test]
async fn insert_and_select() {
    let (_guard, db) = setup().await;

    let created = db
        .insert(users)
        .value(users.insert("oscar@example.com".into(), "Oscar".into(), 21))
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

    assert_eq!(
        sql.sql(),
        r#"SELECT "users"."id", "users"."email", "users"."name", "users"."age", "users"."bio" FROM "users" WHERE ("users"."age" > $1) AND ("users"."bio" IS NULL) ORDER BY "users"."name" ASC LIMIT $2"#
    );
    assert_eq!(sql.args().len(), 2);
}

#[tokio::test]
async fn filters_and_ordering() {
    let (_guard, db) = setup().await;
    for (name, age) in [("Alice", 30), ("Bob", 17), ("Carol", 25)] {
        db.insert(users)
            .value(users.insert(
                format!("{name}@example.com"),
                name.to_string(),
                age,
            ))
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
    let (_guard, db) = setup().await;
    for (name, age) in [("Alice", 30), ("Bob", 20)] {
        db.insert(users)
            .value(users.insert(
                format!("{name}@example.com"),
                name.to_string(),
                age,
            ))
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
    let (_guard, db) = setup().await;
    db.insert(users)
        .value(users.insert("a@example.com".into(), "Alice".into(), 30))
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
    let (_guard, db) = setup().await;
    let author = db
        .insert(users)
        .value(users.insert("a@example.com".into(), "Alice".into(), 30))
        .returning(users)
        .await
        .unwrap()
        .remove(0);
    db.insert(posts)
        .value(posts.insert("Hello".into(), author.id))
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
    let (_guard, db) = setup().await;
    db.insert(users)
        .value(users.insert("a@example.com".into(), "Alice".into(), 30))
        .await
        .unwrap();

    let rows = db
        .insert(users)
        .value(users.insert("a@example.com".into(), "Alice".into(), 31))
        .on_conflict(users.email)
        .do_update(users.age, 31)
        .returning(users)
        .await
        .unwrap();
    assert_eq!(rows[0].age, 31);
}

#[tokio::test]
async fn transactions() {
    let (_guard, db) = setup().await;

    db.transaction(async |tx| {
        tx.insert(users)
            .value(users.insert("a@example.com".into(), "Alice".into(), 30))
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
    let (_guard, db) = setup().await;
    db.insert(users)
        .value(users.insert("a@example.com".into(), "Alice".into(), 30))
        .await
        .unwrap();

    // `Fragment` rewrites the source `?` placeholders to the dialect's spelling.
    let rendered = db
        .execute(sql!("SELECT * FROM users WHERE age > ?", 18))
        .to_sql()
        .unwrap();
    assert_eq!(rendered.sql(), "SELECT * FROM users WHERE age > $1");

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
async fn null_binds_to_any_type() {
    let (_guard, db) = setup().await;

    // `Value::Null` carries no type, so the driver sends it untyped and PostgreSQL accepts it
    // for a `uuid` column even though no `uuid` value was ever bound.
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
    let (_guard, db) = setup().await;
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
    let _guard = lock();
    let db = database().await;
    drop_tables(&db).await;

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
