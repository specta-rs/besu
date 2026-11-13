//! The native `rusqlite` driver, end to end.
//!
//! Everything here runs against `:memory:` unless a test is specifically about a file,
//! because the native driver has one connection: `:memory:` is one database, not one per
//! statement.
#![cfg(feature = "native")]
#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_arguments
)]

use besu::Error;
use besu::prelude::*;
use besu::schema::{Migrator, Schema};
use besu_sqlite::{Native, NativeDatabase, NativeError};

async fn memory() -> NativeDatabase {
    Native::in_memory().unwrap().database()
}

// ---------------------------------------------------------------------------------------
// The everyday path: DDL, insert, returning, select, joins and nulls.
// ---------------------------------------------------------------------------------------

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

#[tokio::test]
async fn ddl_insert_select_returning_and_nulls() {
    let db = memory().await;

    let created = db.push([users.def(), posts.def()]).await.unwrap();
    assert!(
        created
            .statements
            .iter()
            .any(|statement| statement.contains("AUTOINCREMENT")),
        "a SQLite integer primary key must be inline: {:?}",
        created.statements
    );

    let alice = db
        .insert(users)
        .value(users.insert("a@example.com", "Alice", 30))
        .returning(users)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(alice.id, 1);
    assert_eq!(alice.bio, None, "an omitted nullable column is NULL");

    db.insert(users)
        .value(users.insert("b@example.com", "Bob", 25).bio("Rust"))
        .await
        .unwrap();
    db.insert(posts)
        .value(posts.insert("Hello", alice.id))
        .await
        .unwrap();

    let adults: Vec<User> = db
        .select(users)
        .filter(users.age.gte(30))
        .order_by(users.id.asc())
        .await
        .unwrap();
    assert_eq!(adults.len(), 1);
    assert_eq!(adults[0].name, "Alice");

    let with_bio: Vec<String> = db
        .select(users)
        .filter(users.bio.is_not_null())
        .pick(users.name)
        .await
        .unwrap();
    assert_eq!(with_bio, ["Bob"]);

    let joined: Vec<(String, String)> = db
        .select(users)
        .inner_join(posts, eq(users.id, posts.author_id))
        .pick((users.name, posts.title))
        .await
        .unwrap();
    assert_eq!(joined, [("Alice".to_string(), "Hello".to_string())]);

    // The outer side of a left join is NULL, which `Option<T>` peeks through `is_null`.
    let outer: Vec<(String, Option<String>)> = db
        .select(users)
        .left_join(posts, eq(users.id, posts.author_id))
        .pick((users.name, posts.title))
        .order_by(users.id.asc())
        .await
        .unwrap();
    assert_eq!(
        outer,
        [
            ("Alice".to_string(), Some("Hello".to_string())),
            ("Bob".to_string(), None)
        ]
    );

    // An empty result still carries the column names of the statement.
    let none: Vec<User> = db.select(users).filter(users.age.gt(1_000)).await.unwrap();
    assert!(none.is_empty());
}

// ---------------------------------------------------------------------------------------
// Every type the sqlx driver decodes, decoded by the native driver.
// ---------------------------------------------------------------------------------------

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "things")]
pub struct Thing {
    #[besu(primary_key)]
    pub id: uuid::Uuid,
    pub flag: bool,
    pub tiny: i8,
    pub small: i16,
    pub normal: i32,
    pub big: i64,
    pub unsigned_tiny: u8,
    pub unsigned_small: u16,
    pub unsigned_normal: u32,
    pub unsigned_big: u64,
    pub ratio: f32,
    pub precise: f64,
    pub text: String,
    pub blob: Vec<u8>,
    pub when_date: chrono::NaiveDate,
    pub when_time: chrono::NaiveTime,
    pub when_timestamp: chrono::NaiveDateTime,
    pub when_timestamp_tz: chrono::DateTime<chrono::Utc>,
    pub document: serde_json::Value,
    pub maybe: Option<String>,
}

#[tokio::test]
async fn every_type_round_trips() {
    let db = memory().await;
    db.push([things.def()]).await.unwrap();

    let id = uuid::Uuid::new_v4();
    let original = Thing {
        id,
        flag: true,
        tiny: -8,
        small: -1_000,
        normal: -100_000,
        big: -10_000_000_000,
        unsigned_tiny: 200,
        unsigned_small: 60_000,
        unsigned_normal: 4_000_000_000,
        unsigned_big: 10_000_000_000,
        ratio: 1.5,
        precise: 2.25,
        text: "hello".into(),
        blob: vec![1, 2, 3, 255],
        when_date: chrono::NaiveDate::from_ymd_opt(2024, 5, 17).unwrap(),
        when_time: chrono::NaiveTime::from_hms_opt(13, 45, 6).unwrap(),
        when_timestamp: chrono::NaiveDateTime::new(
            chrono::NaiveDate::from_ymd_opt(2024, 5, 17).unwrap(),
            chrono::NaiveTime::from_hms_nano_opt(13, 45, 6, 123_456_789).unwrap(),
        ),
        when_timestamp_tz: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        document: serde_json::json!({ "a": 1, "b": [true, null] }),
        maybe: None,
    };

    db.insert(things)
        .value(things.insert(
            id,
            original.flag,
            original.tiny,
            original.small,
            original.normal,
            original.big,
            original.unsigned_tiny,
            original.unsigned_small,
            original.unsigned_normal,
            original.unsigned_big,
            original.ratio,
            original.precise,
            original.text.clone(),
            original.blob.clone(),
            original.when_date,
            original.when_time,
            original.when_timestamp,
            original.when_timestamp_tz,
            original.document.clone(),
        ))
        .await
        .unwrap();

    let stored: Vec<Thing> = db.select(things).await.unwrap();
    assert_eq!(stored, [original]);
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "wide_numbers")]
pub struct WideNumber {
    #[besu(primary_key)]
    pub id: i64,
    pub value: u128,
    pub signed: i128,
    pub count: u64,
}

#[tokio::test]
async fn wide_integers_round_trip_at_the_extremes() {
    let db = memory().await;
    db.push([wide_numbers.def()]).await.unwrap();

    // `u64::MAX / 2` is `i64::MAX`, the largest unsigned value SQLite's integer storage can
    // hold. `u128` has no such limit because the column is decimal text.
    for (id, (value, signed, count)) in [
        (0u128, 0i128, 0u64),
        (1, -1, 1),
        (u64::MAX as u128, i128::MIN, u64::MAX / 2),
        (u128::MAX, i128::MAX, u64::MAX / 2),
    ]
    .into_iter()
    .enumerate()
    {
        db.insert(wide_numbers)
            .value(wide_numbers.insert(id as i64, value, signed, count))
            .await
            .unwrap();

        let stored: Vec<WideNumber> = db
            .select(wide_numbers)
            .filter(wide_numbers.id.eq(id as i64))
            .await
            .unwrap();
        assert_eq!(stored[0].value, value);
        assert_eq!(stored[0].signed, signed);
        assert_eq!(stored[0].count, count);
    }

    // A `u64` that does not fit in SQLite's signed integer is an encode error, not a wrap.
    let error = db
        .insert(wide_numbers)
        .value(wide_numbers.insert(99_i64, 0_u128, 0_i128, u64::MAX))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("does not fit"), "{error}");
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, besu::SqlEnum)]
#[besu(rename_all = "snake_case")]
pub enum Mood {
    Happy,
    #[besu(rename = "meh")]
    Neutral,
    Sad,
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "reviews")]
pub struct Review {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    #[besu(sql_type = "Mood")]
    pub mood: Mood,
}

#[tokio::test]
async fn enums_read_their_label_through_decode_text() {
    let db = memory().await;
    db.push([reviews.def()]).await.unwrap();

    db.insert(reviews)
        .value(reviews.insert(Mood::Neutral))
        .await
        .unwrap();

    let stored: Vec<Review> = db
        .select(reviews)
        .filter(reviews.mood.eq(Mood::Neutral))
        .await
        .unwrap();
    assert_eq!(stored[0].mood, Mood::Neutral);

    // `decode_text` is the path the `SqlEnum` derive uses; an unknown label is an error
    // rather than an invented variant.
    db.execute(sql!("INSERT INTO reviews (mood) VALUES ('angry')"))
        .await
        .unwrap();
    let error = db.select(reviews).await.unwrap_err();
    assert!(error.to_string().contains("angry"), "{error}");
    assert!(error.to_string().contains("column 1"), "{error}");
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "extended")]
pub struct Extended {
    #[besu(primary_key)]
    pub id: i64,
    pub span: besu::SqlInterval,
    pub host: besu::Inet,
    pub network: besu::Cidr,
    #[besu(sql_type = "besu::Bit<5>")]
    pub flags: besu::BitString,
}

#[tokio::test]
async fn the_text_types_round_trip() {
    let db = memory().await;
    db.push([extended.def()]).await.unwrap();

    let original = Extended {
        id: 1,
        span: besu::SqlInterval::new(2, 3, 4 * 3_600_000_000 + 5 * 60_000_000),
        host: besu::Inet::new("10.0.0.1/8").unwrap(),
        network: besu::Cidr::new("192.168.1.0/24").unwrap(),
        flags: besu::BitString::new("10110").unwrap(),
    };
    db.insert(extended)
        .value(extended.insert(
            original.id,
            original.span,
            original.host.clone(),
            original.network.clone(),
            original.flags.clone(),
        ))
        .await
        .unwrap();

    let stored: Vec<Extended> = db.select(extended).await.unwrap();
    assert_eq!(stored, [original]);
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "money")]
pub struct Money {
    #[besu(primary_key)]
    pub id: i64,
    #[besu(sql_type = "besu::Decimal<10, 2>")]
    pub amount: String,
}

#[tokio::test]
async fn a_decimal_is_exact_text() {
    let db = memory().await;
    db.push([money.def()]).await.unwrap();

    for (id, amount) in [(1_i64, "0.10"), (2, "12.34")] {
        db.insert(money)
            .value(money.insert(id, amount.to_owned()))
            .await
            .unwrap();
    }

    let stored: Vec<Money> = db.select(money).order_by(money.id.asc()).await.unwrap();
    assert_eq!(
        stored
            .iter()
            .map(|row| row.amount.as_str())
            .collect::<Vec<_>>(),
        ["0.10", "12.34"]
    );
}

/// SQLite has no array type, so a bound array is rejected before it reaches the database.
#[tokio::test]
async fn an_array_is_rejected() {
    let db = memory().await;
    let statement = besu::Fragment::with_args(
        "SELECT ?",
        vec![Value::Array {
            element: TypeDef::Text,
            values: vec![Value::Text("a".into())],
        }],
    );
    let error = db.execute(statement).await.unwrap_err();
    assert!(error.to_string().contains("no array type"), "{error}");
}

// ---------------------------------------------------------------------------------------
// Transactions, savepoints and streaming.
// ---------------------------------------------------------------------------------------

async fn count(db: &NativeDatabase) -> i64 {
    db.select(users).pick(count_all()).await.unwrap().remove(0)
}

async fn users_database() -> NativeDatabase {
    let db = memory().await;
    db.push([users.def()]).await.unwrap();
    db
}

#[tokio::test]
async fn a_transaction_commits_and_rolls_back() {
    let db = users_database().await;

    db.transaction(async |tx| {
        tx.insert(users)
            .value(users.insert("committed@example.com", "Committed", 1))
            .await?;
        Ok::<(), NativeError>(())
    })
    .await
    .unwrap();
    assert_eq!(count(&db).await, 1);

    // An error from the closure rolls the whole transaction back.
    let error = db
        .transaction(async |tx| {
            tx.insert(users)
                .value(users.insert("rolled@example.com", "Rolled", 2))
                .await?;
            Err::<(), _>(NativeError::Core(Error::invalid_query("deliberate")))
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("deliberate"));
    assert_eq!(count(&db).await, 1);

    // An explicit handle that is rolled back behaves the same way.
    let mut tx = db.begin().await.unwrap();
    tx.insert(users)
        .value(users.insert("explicit@example.com", "Explicit", 3))
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(count(&db).await, 1);

    // Dropping a handle without committing also rolls back.
    {
        let mut tx = db.begin().await.unwrap();
        tx.insert(users)
            .value(users.insert("dropped@example.com", "Dropped", 4))
            .await
            .unwrap();
    }
    assert_eq!(count(&db).await, 1);
}

#[tokio::test]
async fn a_failed_savepoint_keeps_the_outer_work() {
    let db = users_database().await;

    db.transaction(async |tx| {
        tx.insert(users)
            .value(users.insert("outer@example.com", "Outer", 30))
            .await?;

        let inner: Result<(), _> = tx
            .transaction(async |tx| {
                tx.insert(users)
                    .value(users.insert("inner@example.com", "Inner", 31))
                    .await?;
                Err::<(), _>(NativeError::Core(Error::invalid_query("deliberate")))
            })
            .await;
        assert!(inner.is_err());

        Ok::<(), NativeError>(())
    })
    .await
    .unwrap();

    let emails: Vec<String> = db
        .select(users)
        .pick(users.email)
        .order_by(users.id.asc())
        .await
        .unwrap();
    assert_eq!(emails, ["outer@example.com"]);
}

#[tokio::test]
async fn a_stream_delivers_every_row() {
    let db = users_database().await;
    for (index, name) in ["One", "Two", "Three"].into_iter().enumerate() {
        db.insert(users)
            .value(users.insert(
                format!("{index}@example.com"),
                name.to_string(),
                index as i32,
            ))
            .await
            .unwrap();
    }

    let statement = db.select(users).order_by(users.id.asc()).to_sql().unwrap();
    let rows: Vec<User> = db
        .stream(&statement)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(
        rows.iter()
            .map(|user| user.name.as_str())
            .collect::<Vec<_>>(),
        ["One", "Two", "Three"]
    );

    // Streaming inside a transaction goes through the same connection.
    db.transaction(async |tx| {
        let statement = tx.select(users).order_by(users.id.desc()).to_sql().unwrap();
        let rows = tx.stream::<User>(&statement).await?.collect().await?;
        assert_eq!(rows[0].name, "Three");
        Ok::<(), NativeError>(())
    })
    .await
    .unwrap();
}

// ---------------------------------------------------------------------------------------
// Introspection, push and migrations.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn introspection_round_trips_and_push_converges() {
    let db = memory().await;

    let first = db.push([users.def(), posts.def()]).await.unwrap();
    assert!(first.statements[0].contains("CREATE TABLE \"users\""));

    let second = db.push([users.def(), posts.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);

    let schema: Schema = db.introspect().await.unwrap();
    assert_eq!(schema.tables.len(), 2);

    let users_table = schema.table("users").unwrap();
    assert!(
        users_table
            .columns
            .iter()
            .any(|column| column.name == "id" && column.auto_increment && column.primary_key)
    );
    assert!(
        users_table
            .columns
            .iter()
            .any(|column| column.name == "email" && column.unique)
    );
    assert!(
        users_table
            .columns
            .iter()
            .any(|column| column.name == "bio" && column.nullable)
    );

    let posts_table = schema.table("posts").unwrap();
    assert_eq!(posts_table.foreign_keys.len(), 1);
    assert_eq!(posts_table.foreign_keys[0].references_table, "users");

    // A schema read back from the database is a no-op when pushed.
    let pushed = db.push_schema(&schema).await.unwrap();
    assert!(pushed.is_empty(), "got {:?}", pushed.changes);
}

/// Two declarations of one table, so a push has a column to add. Each lives in its own
/// module because the derive names the table handle after the table, and both are `people`.
// The derive re-exports the model from inside the module; nothing outside names it through
// that path, so the re-export is legitimately unused here.
#[allow(unused_imports)]
mod person_v1 {
    use super::*;

    #[derive(Table, Debug, Clone, PartialEq)]
    #[besu(table = "people")]
    pub struct PersonV1 {
        #[besu(primary_key, auto_increment)]
        pub id: i64,
        pub name: String,
    }
}

#[allow(unused_imports)]
mod person_v2 {
    use super::*;

    #[derive(Table, Debug, Clone, PartialEq)]
    #[besu(table = "people")]
    pub struct PersonV2 {
        #[besu(primary_key, auto_increment)]
        pub id: i64,
        pub name: String,
        pub nickname: Option<String>,
    }
}

use person_v1::{PersonV1, people as person_old};
use person_v2::{PersonV2, people as person_new};

#[tokio::test]
async fn push_creates_and_then_alters_a_table() {
    let db = memory().await;

    let first = db.push([person_old.def()]).await.unwrap();
    assert!(first.statements[0].contains("CREATE TABLE \"people\""));

    db.insert(person_old)
        .value(person_old.insert("Ada"))
        .await
        .unwrap();
    let before: Vec<PersonV1> = db.select(person_old).await.unwrap();
    assert_eq!(before[0].name, "Ada");

    let second = db.push([person_new.def()]).await.unwrap();
    assert!(
        second
            .statements
            .iter()
            .any(|statement| statement.contains("ADD COLUMN \"nickname\"")),
        "the new column must be added: {:?}",
        second.statements
    );

    // The row that existed before the change is still there, with a NULL for the new column.
    let stored: Vec<PersonV2> = db.select(person_new).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].name, "Ada");
    assert_eq!(stored[0].nickname, None);

    let converged = db.push([person_new.def()]).await.unwrap();
    assert!(converged.is_empty(), "got {:?}", converged.changes);
}

/// A third and fourth declaration, this time changing a check. SQLite has no
/// `ALTER TABLE ... ADD CONSTRAINT`, so the differ rebuilds the table: several statements
/// with a `PRAGMA foreign_keys` toggle that only works if they share one connection.
#[allow(unused_imports)]
mod account_v1 {
    use super::*;

    #[derive(Table, Debug, Clone, PartialEq)]
    #[besu(table = "accounts")]
    pub struct AccountV1 {
        #[besu(primary_key, auto_increment)]
        pub id: i64,
        pub balance: i64,
    }
}

#[allow(unused_imports)]
mod account_v2 {
    use super::*;

    #[derive(Table, Debug, Clone, PartialEq)]
    #[besu(table = "accounts")]
    #[besu(check = "balance >= 0")]
    pub struct AccountV2 {
        #[besu(primary_key, auto_increment)]
        pub id: i64,
        pub balance: i64,
    }
}

#[tokio::test]
async fn push_rebuilds_a_table_in_one_request() {
    use account_v1::AccountV1;
    use account_v2::AccountV2;

    let db = memory().await;
    db.push([account_v1::accounts.def()]).await.unwrap();
    db.insert(account_v1::accounts)
        .value(account_v1::accounts.insert(10))
        .await
        .unwrap();
    let before: Vec<AccountV1> = db.select(account_v1::accounts).await.unwrap();
    assert_eq!(before[0].balance, 10);

    let rebuilt = db.push([account_v2::accounts.def()]).await.unwrap();
    assert!(
        rebuilt
            .statements
            .iter()
            .any(|statement| statement.contains("PRAGMA foreign_keys = OFF")),
        "a check change must rebuild the table: {:?}",
        rebuilt.statements
    );
    assert!(
        rebuilt
            .statements
            .iter()
            .any(|statement| statement.contains("PRAGMA foreign_keys = ON")),
        "the rebuild must restore the pragma: {:?}",
        rebuilt.statements
    );

    // The rebuild copied the row across, and the new schema converged.
    let rows: Vec<AccountV2> = db.select(account_v2::accounts).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].balance, 10);

    assert!(
        db.push([account_v2::accounts.def()])
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn migrate_from_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("0001_create_users.sql"),
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL);",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("0002_add_email.sql"),
        "ALTER TABLE users ADD COLUMN email TEXT;\n\
         CREATE UNIQUE INDEX users_email_idx ON users (email);",
    )
    .unwrap();

    let migrator = Migrator::from_dir(dir.path()).unwrap();
    let db = memory().await;

    let applied = db.migrate(&migrator).await.unwrap();
    assert_eq!(
        applied.iter().map(|m| m.version).collect::<Vec<_>>(),
        [1, 2]
    );

    // The journal remembers what ran, and it is not part of the introspected schema.
    assert!(db.migrate(&migrator).await.unwrap().is_empty());
    assert!(
        db.introspect()
            .await
            .unwrap()
            .table("_besu_migrations")
            .is_none()
    );

    db.execute(sql!(
        "INSERT INTO users (id, name, email) VALUES (?, ?, ?)",
        1,
        "Oscar",
        "o@example.com"
    ))
    .await
    .unwrap();
    let rows = db
        .execute(sql!(
            "SELECT name FROM users WHERE email = ?",
            "o@example.com"
        ))
        .all::<(String,)>()
        .await
        .unwrap();
    assert_eq!(rows, [("Oscar".to_string(),)]);
}

#[tokio::test]
async fn a_file_database_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.db");

    {
        let db = Native::new(&path).unwrap().database();
        db.push([users.def()]).await.unwrap();
        db.insert(users)
            .value(users.insert("file@example.com", "File", 42))
            .await
            .unwrap();
    }

    // The first driver, and its connection thread, are gone; the file is what is left.
    let db = Native::open(&path).unwrap().database();
    let stored: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].email, "file@example.com");
    assert_eq!(stored[0].age, 42);
}

// ---------------------------------------------------------------------------------------
// Introspection and rebuild regressions.
// ---------------------------------------------------------------------------------------

/// Two foreign keys can start on the same column, so `PRAGMA foreign_key_list` reports them
/// as two constraints whose rows differ only in their `id`. Grouping them by the name derived
/// from the column merged them into one malformed constraint and lost a target.
#[tokio::test]
async fn two_foreign_keys_on_one_column_stay_apart() {
    let db = memory().await;
    db.execute(sql!("CREATE TABLE a (id INTEGER PRIMARY KEY)"))
        .await
        .unwrap();
    db.execute(sql!("CREATE TABLE b (id INTEGER PRIMARY KEY)"))
        .await
        .unwrap();
    db.execute(sql!(
        "CREATE TABLE links (\
           id INTEGER PRIMARY KEY, \
           target INTEGER NOT NULL, \
           CONSTRAINT links_one FOREIGN KEY (target) REFERENCES a (id), \
           CONSTRAINT links_two FOREIGN KEY (target) REFERENCES b (id))"
    ))
    .await
    .unwrap();

    let schema: Schema = db.introspect().await.unwrap();
    let table = schema.table("links").unwrap();
    assert_eq!(table.foreign_keys.len(), 2, "{:?}", table.foreign_keys);
    assert_eq!(
        table
            .foreign_keys
            .iter()
            .map(|fk| (fk.columns.clone(), fk.references_table.as_str()))
            .collect::<Vec<_>>(),
        [
            (vec!["target".to_owned()], "a"),
            (vec!["target".to_owned()], "b"),
        ]
    );
    assert_ne!(
        table.foreign_keys[0].name, table.foreign_keys[1].name,
        "two constraints on one table need two names"
    );

    // The snapshot it read is itself a no-op against the database it came from.
    assert!(db.push_schema(&schema).await.unwrap().is_empty());

    // And it recreates the same two constraints in a database that has none.
    let fresh = memory().await;
    fresh.push_schema(&schema).await.unwrap();
    let reread: Schema = fresh.introspect().await.unwrap();
    assert_eq!(
        reread
            .table("links")
            .unwrap()
            .foreign_keys
            .iter()
            .map(|fk| (fk.columns.clone(), fk.references_table.as_str()))
            .collect::<Vec<_>>(),
        [
            (vec!["target".to_owned()], "a"),
            (vec!["target".to_owned()], "b"),
        ]
    );
}

/// A foreign key written without a parent column list matches the parent's primary key, which
/// `PRAGMA foreign_key_list` reports as `NULL`. Guessing `id` produced a constraint over a
/// column that need not exist.
#[tokio::test]
async fn a_foreign_key_without_a_parent_column_list_reads_the_primary_key() {
    let db = memory().await;
    db.execute(sql!(
        "CREATE TABLE parents (key INTEGER PRIMARY KEY, name TEXT NOT NULL)"
    ))
    .await
    .unwrap();
    db.execute(sql!(
        "CREATE TABLE kids (id INTEGER PRIMARY KEY, parent_key INTEGER REFERENCES parents)"
    ))
    .await
    .unwrap();

    let schema: Schema = db.introspect().await.unwrap();
    let foreign_keys = &schema.table("kids").unwrap().foreign_keys;
    assert_eq!(foreign_keys.len(), 1);
    assert_eq!(foreign_keys[0].references_columns, ["key"]);

    // A database built from that snapshot enforces the constraint against the right column,
    // which it could not do if the snapshot had guessed `parents.id`.
    let fresh = memory().await;
    fresh.push_schema(&schema).await.unwrap();
    fresh
        .execute(sql!("INSERT INTO parents (key, name) VALUES (1, 'one')"))
        .await
        .unwrap();
    fresh
        .execute(sql!("INSERT INTO kids (id, parent_key) VALUES (1, 1)"))
        .await
        .unwrap();
    let error = fresh
        .execute(sql!("INSERT INTO kids (id, parent_key) VALUES (2, 2)"))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("FOREIGN KEY constraint failed"),
        "{error}"
    );
}

/// A failed rebuild used to leave `PRAGMA foreign_keys = OFF` on the connection, so every
/// later statement silently accepted a violation. It also left its scratch table behind,
/// which the next push reported as a destructive drop.
#[tokio::test]
async fn a_failed_rebuild_restores_foreign_keys_and_can_be_retried() {
    let db = memory().await;
    db.execute(sql!("CREATE TABLE parent (id INTEGER PRIMARY KEY)"))
        .await
        .unwrap();
    db.execute(sql!("INSERT INTO parent (id) VALUES (1)"))
        .await
        .unwrap();
    db.execute(sql!(
        "CREATE TABLE child (id INTEGER PRIMARY KEY, parent_id INTEGER NOT NULL REFERENCES parent (id))"
    ))
    .await
    .unwrap();
    db.execute(sql!(
        "CREATE TABLE people (id INTEGER PRIMARY KEY, nickname TEXT)"
    ))
    .await
    .unwrap();
    db.execute(sql!("INSERT INTO people (id, nickname) VALUES (1, 'Ada')"))
        .await
        .unwrap();

    // A rebuild (the nullable change) plus a new `NOT NULL` column with no default: the copy
    // has no value for the column, so the rebuild fails after it created its scratch table.
    let current: Schema = db.introspect().await.unwrap();
    let mut doomed = current.clone();
    let table = doomed.table_mut("people").unwrap();
    table.columns[1].nullable = false;
    let mut required = table.columns[0].clone();
    required.name = "required".to_owned();
    required.nullable = false;
    required.primary_key = false;
    required.auto_increment = false;
    table.columns.push(required);

    assert!(db.push_schema(&doomed).await.is_err());

    // Foreign keys are still enforced.
    let enabled: Vec<(i64,)> = db.execute(sql!("PRAGMA foreign_keys")).all().await.unwrap();
    assert_eq!(enabled, [(1,)], "a failed rebuild must restore the pragma");
    let violation = db
        .execute(sql!("INSERT INTO child (id, parent_id) VALUES (9, 999)"))
        .await
        .unwrap_err();
    assert!(
        violation
            .to_string()
            .contains("FOREIGN KEY constraint failed"),
        "{violation}"
    );

    // The scratch table is not part of the schema while the table it copies still exists, so
    // a retry with a valid diff is not reported as a destructive drop.
    let retried: Schema = db.introspect().await.unwrap();
    assert!(retried.table("people__besu_tmp").is_none());

    let mut fixed = current.clone();
    fixed.table_mut("people").unwrap().columns[1].nullable = false;
    db.push_schema(&fixed).await.unwrap();

    let rows: Vec<(i64, String)> = db
        .execute(sql!("SELECT id, nickname FROM people"))
        .all()
        .await
        .unwrap();
    assert_eq!(rows, [(1, "Ada".to_owned())]);
    let leftover: Vec<(String,)> = db
        .execute(sql!(
            "SELECT name FROM sqlite_master WHERE name LIKE '%__besu_tmp'"
        ))
        .all()
        .await
        .unwrap();
    assert!(leftover.is_empty(), "{leftover:?}");
}

/// A rebuild performs a declared rename by copying the old column into the new one. Selecting
/// the new name from the old table dropped the column from the copy, so its values became the
/// new column's default instead of the data.
#[tokio::test]
async fn a_rebuild_keeps_a_renamed_columns_data() {
    // The derive re-exports the model from inside each module; nothing outside names it
    // through that path, so the re-export is legitimately unused.
    #[allow(unused_imports)]
    mod members_v1 {
        use besu::Table;

        #[derive(Table, Debug, Clone)]
        #[besu(table = "members")]
        pub struct Member {
            #[besu(primary_key, auto_increment)]
            pub id: i64,
            pub full_name: String,
        }
    }

    #[allow(unused_imports)]
    mod members_v2 {
        use besu::Table;

        #[derive(Table, Debug, Clone)]
        #[besu(table = "members")]
        #[besu(check = "length(name) > 0")]
        pub struct Member {
            #[besu(primary_key, auto_increment)]
            pub id: i64,
            #[besu(column = "name", renamed_from = "full_name", default = "unknown")]
            pub name: String,
        }
    }

    let db = memory().await;
    db.push([members_v1::members.def()]).await.unwrap();
    db.execute(sql!(
        "INSERT INTO members (id, full_name) VALUES (1, 'Ada')"
    ))
    .await
    .unwrap();

    // The check is added in the same push, which is what makes the differ rebuild the table
    // rather than issue a plain `RENAME COLUMN`.
    let result = db.push([members_v2::members.def()]).await.unwrap();
    assert!(
        result
            .statements
            .iter()
            .any(|statement| statement.contains("__besu_tmp")),
        "expected a rebuild: {:?}",
        result.statements
    );

    let rows: Vec<(i64, String)> = db
        .execute(sql!("SELECT id, name FROM members"))
        .all()
        .await
        .unwrap();
    assert_eq!(
        rows,
        [(1, "Ada".to_owned())],
        "the rename must keep the value, not fall back to the default"
    );

    // The check the rebuild added is enforced.
    let error = db
        .execute(sql!("INSERT INTO members (id, name) VALUES (2, '')"))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("CHECK constraint failed"),
        "{error}"
    );
}

/// SQLite reports `DEFAULT (1 + 1)` as the bare expression, which is only valid inside
/// parentheses. Dropping them turned a table rebuild into a syntax error.
#[tokio::test]
async fn a_rebuild_keeps_an_expression_default() {
    let db = memory().await;
    db.execute(sql!(
        "CREATE TABLE computed (\
           id INTEGER PRIMARY KEY, \
           total INTEGER DEFAULT (1 + 1), \
           created_at TEXT DEFAULT (datetime('now')))"
    ))
    .await
    .unwrap();

    let schema: Schema = db.introspect().await.unwrap();
    let mut desired = schema.clone();
    // A nullable change forces the rebuild without touching the defaults.
    desired.table_mut("computed").unwrap().columns[1].nullable = false;
    let pushed = db.push_schema(&desired).await.unwrap();
    assert!(
        pushed
            .statements
            .iter()
            .any(|statement| statement.contains("__besu_tmp")),
        "expected a rebuild: {:?}",
        pushed.statements
    );

    db.execute(sql!("INSERT INTO computed (id) VALUES (1)"))
        .await
        .unwrap();
    let rows: Vec<(i64, String)> = db
        .execute(sql!("SELECT total, created_at FROM computed"))
        .all()
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, 2);
    assert!(rows[0].1.starts_with("20"), "{}", rows[0].1);
}

/// A `Value::Json` that is not valid UTF-8 is not a JSON document. Binding it as `NOTHING`
/// used to store `"null"`, silently replacing the value instead of reporting the mistake.
#[tokio::test]
async fn a_json_parameter_that_is_not_utf8_is_rejected() {
    use besu::Driver as _;
    use besu_core::{Sql, Value};

    let native = Native::in_memory().unwrap();
    let mut statement = Sql::<besu_sqlite::SQLite>::raw("SELECT ?");
    statement.push_arg(Value::Json(vec![b'{', 0xff, b'}']));
    let statement = statement.finish().unwrap();

    let error = native.query(&statement).await.unwrap_err();
    assert!(error.to_string().contains("not valid UTF-8"), "{error}");
}

/// SQLite has no `ALTER TABLE ... ADD CONSTRAINT`, so a foreign key added to an existing
/// table has to be applied by a rebuild. Without one the change rendered as an empty
/// statement: `push` reported it as applied, but the database never had the constraint.
#[tokio::test]
async fn a_foreign_key_added_by_a_push_is_created() {
    #[allow(unused_imports)]
    mod posts_v1 {
        use besu::Table;

        #[derive(Table, Debug, Clone)]
        #[besu(table = "posts")]
        pub struct Post {
            #[besu(primary_key, auto_increment)]
            pub id: i64,
            pub title: String,
            pub author_id: i64,
        }
    }

    #[allow(unused_imports)]
    mod posts_v2 {
        use besu::Table;

        #[derive(Table, Debug, Clone)]
        #[besu(table = "posts")]
        pub struct Post {
            #[besu(primary_key, auto_increment)]
            pub id: i64,
            pub title: String,
            #[besu(references = "users.id")]
            pub author_id: i64,
        }
    }

    let db = memory().await;
    db.push([users.def(), posts_v1::posts.def()]).await.unwrap();
    db.execute(sql!(
        "INSERT INTO users (id, email, name, age) VALUES (1, 'a@example.com', 'A', 1)"
    ))
    .await
    .unwrap();
    db.execute(sql!(
        "INSERT INTO posts (id, title, author_id) VALUES (1, 'Hello', 1)"
    ))
    .await
    .unwrap();

    let added = db.push([users.def(), posts_v2::posts.def()]).await.unwrap();
    assert!(
        added
            .statements
            .iter()
            .any(|statement| statement.contains("__besu_tmp")),
        "adding a foreign key must rebuild the table: {:?}",
        added.statements
    );

    // The constraint is enforced, and the row that was already there kept its values.
    let error = db
        .execute(sql!(
            "INSERT INTO posts (id, title, author_id) VALUES (2, 'Orphan', 999)"
        ))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("FOREIGN KEY constraint failed"),
        "{error}"
    );
    let titles: Vec<(String,)> = db
        .execute(sql!("SELECT title FROM posts"))
        .all()
        .await
        .unwrap();
    assert_eq!(titles, [("Hello".to_owned(),)]);

    assert!(
        db.push([users.def(), posts_v2::posts.def()])
            .await
            .unwrap()
            .is_empty()
    );
}

/// The same for a multi-column unique constraint, which the differ reports as a `Unique`
/// entry rather than as a change to one column.
#[tokio::test]
async fn a_multi_column_unique_added_by_a_push_is_created() {
    let db = memory().await;
    db.execute(sql!(
        "CREATE TABLE pairs (a INTEGER NOT NULL, b INTEGER NOT NULL)"
    ))
    .await
    .unwrap();
    db.execute(sql!("INSERT INTO pairs (a, b) VALUES (1, 1)"))
        .await
        .unwrap();

    let current: Schema = db.introspect().await.unwrap();
    let mut desired = current.clone();
    desired
        .table_mut("pairs")
        .unwrap()
        .uniques
        .push(besu::schema::Unique {
            name: "pairs_a_b_key".to_owned(),
            columns: vec!["a".to_owned(), "b".to_owned()],
        });

    let added = db.push_schema(&desired).await.unwrap();
    assert!(
        added
            .statements
            .iter()
            .any(|statement| statement.contains("UNIQUE (\"a\", \"b\")")),
        "adding a unique constraint must rebuild the table: {:?}",
        added.statements
    );

    let error = db
        .execute(sql!("INSERT INTO pairs (a, b) VALUES (1, 1)"))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("UNIQUE constraint failed"),
        "{error}"
    );

    assert!(db.push_schema(&desired).await.unwrap().is_empty());
}
