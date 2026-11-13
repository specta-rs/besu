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
