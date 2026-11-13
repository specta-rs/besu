//! The native `mysql_async` driver, end to end.
//!
//! Everything here runs against the MySQL server the rest of the suite uses. Each test gets
//! its own database, so the binary runs in parallel and can be repeated without cleaning up
//! first. The tests cover the driver surface the sqlx suite cannot: the binary value mapping,
//! real streaming, transactions that own a pooled connection, and the shared introspection
//! path.
#![cfg(feature = "native")]
#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_arguments
)]

use besu::prelude::*;
use besu::schema::{Change, Migrator};
use besu::{
    BitString, Driver as _, EnumDef, EnumLabels, Geometry, GeometryKind, Range, TypeDef, Value,
    Vector,
};
use besu_mysql::{MySql, Native, NativeDatabase, NativeError};

/// The MySQL server the suite runs against.
const SERVER: &str = "mysql://root:besu@localhost:53306";

/// Create an empty database named `name`, replacing any database left over from a previous
/// run.
async fn fresh(name: &str) -> NativeDatabase {
    let admin = besu::Database::new(
        Native::new(SERVER)
            .await
            .expect("the test server must be reachable"),
    );
    admin
        .execute(Fragment::new(format!("DROP DATABASE IF EXISTS `{name}`")))
        .await
        .expect("the old database must be droppable");
    admin
        .execute(Fragment::new(format!("CREATE DATABASE `{name}`")))
        .await
        .expect("the database must be creatable");

    besu::Database::new(
        Native::new(&format!("{SERVER}/{name}"))
            .await
            .expect("the new database must be reachable"),
    )
}

// ---------------------------------------------------------------------------------------
// The everyday path
// ---------------------------------------------------------------------------------------

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "native_users")]
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
#[besu(table = "native_posts")]
pub struct Post {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub title: String,
    #[besu(references = "native_users.id", on_delete = "cascade")]
    pub author_id: i64,
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "native_employees")]
pub struct Employee {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub name: String,
    pub manager_id: Option<i64>,
}
#[tokio::test]
async fn connect_insert_select_join_and_null() {
    let db = fresh("besu_native_everyday").await;

    // The session is pinned. Run a few statements first so the connection has gone back to
    // the pool at least once: a pool that reset connections on return would lose this unless
    // the driver re-applied it (`setup`, not `init`).
    for _ in 0..3 {
        db.execute(sql!("SELECT 1")).all::<(i64,)>().await.unwrap();
    }
    let session: (String, String) = db
        .execute(sql!(
            "SELECT @@session.character_set_client, @@session.time_zone"
        ))
        .first()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(session.0, "utf8mb4");
    assert_eq!(session.1, "+00:00");

    let created = db
        .push([native_users.def(), native_posts.def()])
        .await
        .unwrap();
    assert!(
        created
            .statements
            .iter()
            .any(|statement| statement.starts_with("CREATE TABLE")),
        "got {:?}",
        created.statements
    );

    // A write reports the affected row and the id the auto-increment column allocated.
    let output = db
        .insert(native_users)
        .value(native_users.insert("a@example.com", "Alice", 30))
        .await
        .unwrap();
    assert_eq!(output.rows_affected(), 1);
    assert_eq!(output.last_insert_id(), Some(1));

    db.insert(native_users)
        .value(native_users.insert("b@example.com", "Bob", 25))
        .await
        .unwrap();
    db.insert(native_posts)
        .value(native_posts.insert("Hello", 1))
        .await
        .unwrap();

    let all: Vec<User> = db
        .select(native_users)
        .order_by(native_users.id.asc())
        .await
        .unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].name, "Alice");
    assert_eq!(all[0].bio, None);

    let adults: Vec<User> = db
        .select(native_users)
        .filter(native_users.age.gte(26))
        .await
        .unwrap();
    assert_eq!(adults.len(), 1);
    assert_eq!(adults[0].name, "Alice");

    let found: Vec<(String, String)> = db
        .select(native_posts)
        .inner_join(native_users, eq(native_users.id, native_posts.author_id))
        .pick((native_posts.title, native_users.name))
        .filter(native_users.email.eq("a@example.com"))
        .await
        .unwrap();
    assert_eq!(found, [("Hello".to_owned(), "Alice".to_owned())]);

    // A null filter and a null read through an outer join.
    let no_bio: Vec<User> = db
        .select(native_users)
        .filter(native_users.bio.is_null())
        .await
        .unwrap();
    assert_eq!(no_bio.len(), 2);

    db.insert(native_users)
        .value(native_users.insert("c@example.com", "Cara", 40).bio("Rust"))
        .await
        .unwrap();

    let rows: Vec<(String, Option<String>)> = db
        .select(native_users)
        .left_join(native_posts, eq(native_posts.author_id, native_users.id))
        .pick((native_users.name, native_posts.title))
        .order_by(native_users.id.asc())
        .await
        .unwrap();
    assert_eq!(
        rows,
        [
            ("Alice".to_owned(), Some("Hello".to_owned())),
            ("Bob".to_owned(), None),
            ("Cara".to_owned(), None),
        ]
    );

    // A self join, so a query reads the same table twice through two result aliases. MySQL has
    // no `RETURNING`, so the manager's id is selected back before the reports are inserted.
    db.push([
        native_users.def(),
        native_posts.def(),
        native_employees.def(),
    ])
    .await
    .unwrap();
    db.insert(native_employees)
        .value(native_employees.insert("Boss"))
        .await
        .unwrap();
    let boss: Employee = db
        .select(native_employees)
        .filter(native_employees.name.eq("Boss"))
        .first()
        .await
        .unwrap()
        .unwrap();
    db.insert(native_employees)
        .value(native_employees.insert("Worker").manager_id(boss.id))
        .await
        .unwrap();

    let manager = alias(native_employees, "manager");
    let rows: Vec<(Employee, Option<Employee>)> = db
        .select(native_employees)
        .left_join(
            &manager,
            manager
                .column(native_employees.id)
                .eq(native_employees.manager_id),
        )
        .order_by(native_employees.id)
        .all()
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].1, None);
    assert_eq!(rows[1].1.as_ref().map(|e| e.name.as_str()), Some("Boss"));
}

// ---------------------------------------------------------------------------------------
// Every scalar type
// ---------------------------------------------------------------------------------------

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "native_scalars")]
pub struct Scalars {
    #[besu(primary_key)]
    pub id: i64,
    pub flag: bool,
    pub tiny: i8,
    pub small: i16,
    pub normal: i32,
    pub big: i64,
    pub unsigned_tiny: u8,
    pub unsigned_small: u16,
    pub unsigned_normal: u32,
    pub unsigned_big: u64,
    pub wide: u128,
    pub signed_wide: i128,
    pub ratio: f32,
    pub precise: f64,
    pub text: String,
    pub blob: Vec<u8>,
    pub key: uuid::Uuid,
    pub document: serde_json::Value,
    pub maybe: Option<String>,
}

impl Scalars {
    fn sample(id: i64) -> Self {
        Self {
            id,
            flag: true,
            tiny: -8,
            small: -1_000,
            normal: -100_000,
            big: -10_000_000_000,
            unsigned_tiny: 200,
            unsigned_small: 60_000,
            unsigned_normal: 4_000_000_000,
            unsigned_big: 10_000_000_000_000_000_000,
            wide: u128::MAX,
            signed_wide: i128::MIN,
            ratio: 1.5,
            precise: 2.25,
            text: "hello 🌍".into(),
            blob: vec![1, 2, 3, 255],
            key: uuid::Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef),
            document: serde_json::json!({ "a": 1, "b": [true, null], "c": "é" }),
            maybe: None,
        }
    }
}

#[tokio::test]
async fn every_scalar_type_round_trips() {
    let db = fresh("besu_native_scalars").await;
    db.push([native_scalars.def()]).await.unwrap();

    let expected = Scalars::sample(1);
    db.insert(native_scalars)
        .value(native_scalars.insert(
            expected.id,
            expected.flag,
            expected.tiny,
            expected.small,
            expected.normal,
            expected.big,
            expected.unsigned_tiny,
            expected.unsigned_small,
            expected.unsigned_normal,
            expected.unsigned_big,
            expected.wide,
            expected.signed_wide,
            expected.ratio,
            expected.precise,
            expected.text.clone(),
            expected.blob.clone(),
            expected.key,
            expected.document.clone(),
        ))
        .await
        .unwrap();

    let stored: Vec<Scalars> = db.select(native_scalars).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].flag, expected.flag);
    assert_eq!(stored[0].tiny, expected.tiny);
    assert_eq!(stored[0].small, expected.small);
    assert_eq!(stored[0].normal, expected.normal);
    assert_eq!(stored[0].big, expected.big);
    assert_eq!(stored[0].unsigned_tiny, expected.unsigned_tiny);
    assert_eq!(stored[0].unsigned_small, expected.unsigned_small);
    assert_eq!(stored[0].unsigned_normal, expected.unsigned_normal);
    assert_eq!(stored[0].unsigned_big, expected.unsigned_big);
    assert_eq!(stored[0].wide, expected.wide);
    assert_eq!(stored[0].signed_wide, expected.signed_wide);
    assert_eq!(stored[0].ratio, expected.ratio);
    assert_eq!(stored[0].precise, expected.precise);
    assert_eq!(stored[0].text, expected.text);
    assert_eq!(stored[0].blob, expected.blob);
    assert_eq!(stored[0].key, expected.key);
    assert_eq!(stored[0].document, expected.document);
    assert_eq!(stored[0].maybe, None);
}

/// The 128-bit columns at the extremes, including the values that do not fit in an `i64`.
#[tokio::test]
async fn wide_integers_round_trip_at_the_extremes() {
    let db = fresh("besu_native_wide").await;
    db.push([native_scalars.def()]).await.unwrap();

    for (id, wide) in [0, 1, u64::MAX as u128, u128::MAX].into_iter().enumerate() {
        let id = id as i64;
        let mut value = Scalars::sample(id);
        value.unsigned_big = wide as u64;
        value.wide = wide;
        value.signed_wide = -(wide as i128);
        db.insert(native_scalars)
            .value(native_scalars.insert(
                value.id,
                value.flag,
                value.tiny,
                value.small,
                value.normal,
                value.big,
                value.unsigned_tiny,
                value.unsigned_small,
                value.unsigned_normal,
                value.unsigned_big,
                value.wide,
                value.signed_wide,
                value.ratio,
                value.precise,
                value.text.clone(),
                value.blob.clone(),
                value.key,
                value.document.clone(),
            ))
            .await
            .unwrap();

        let stored: Vec<Scalars> = db
            .select(native_scalars)
            .filter(native_scalars.id.eq(id))
            .await
            .unwrap();
        assert_eq!(stored[0].wide, value.wide);
        assert_eq!(stored[0].signed_wide, value.signed_wide);
        assert_eq!(stored[0].unsigned_big, value.unsigned_big);
    }

    // `u128` is `DECIMAL(39,0)` and `i128` is the same storage with a sign, and both
    // introspect back to the exact same declared type, so a second push is a no-op.
    let schema = besu::Database::<Native>::schema([native_scalars.def()]);
    let table = schema.table("native_scalars").unwrap();
    assert_eq!(table.column("wide").unwrap().ty, TypeDef::UInt(128));
    assert_eq!(
        table.column("signed_wide").unwrap().ty,
        TypeDef::Decimal(39, 0)
    );
    assert!(db.push([native_scalars.def()]).await.unwrap().is_empty());

    // `u64::MAX` cannot be decoded as `i64`, and that is an error rather than a truncation.
    let too_wide = db
        .execute(Fragment::new(
            "SELECT `unsigned_big` FROM `native_scalars` WHERE `unsigned_big` > 0".to_owned(),
        ))
        .all::<(i64,)>()
        .await;
    assert!(too_wide.is_err(), "got {too_wide:?}");
}

// ---------------------------------------------------------------------------------------
// Temporal values and exact decimals
// ---------------------------------------------------------------------------------------

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "native_temporal")]
pub struct Temporal {
    #[besu(primary_key)]
    pub id: i64,
    pub day: chrono::NaiveDate,
    pub clock: chrono::NaiveTime,
    pub stamp: chrono::NaiveDateTime,
    pub stamp_tz: chrono::DateTime<chrono::Utc>,
    #[besu(sql_type = "besu::Decimal<12,2>")]
    pub amount: String,
}

#[tokio::test]
async fn temporal_and_decimal_values_round_trip() {
    let db = fresh("besu_native_temporal").await;
    db.push([native_temporal.def()]).await.unwrap();

    // The `DateTime<Utc>` column is `DATETIME(6)` and introspects back as a timestamp, so a
    // second push must be a no-op.
    let second = db.push([native_temporal.def()]).await.unwrap();
    assert!(second.is_empty(), "got {:?}", second.changes);

    for (id, amount) in [(1_i64, "0.00"), (2, "1.23"), (3, "-1234567890.99")] {
        db.insert(native_temporal)
            .value(
                native_temporal.insert(
                    id,
                    chrono::NaiveDate::from_ymd_opt(2024, 5, 17).unwrap(),
                    chrono::NaiveTime::from_hms_micro_opt(23, 45, 6, 789_000).unwrap(),
                    chrono::NaiveDate::from_ymd_opt(2024, 5, 17)
                        .unwrap()
                        .and_hms_micro_opt(13, 45, 6, 123_456)
                        .unwrap(),
                    chrono::DateTime::from_timestamp(1_700_000_000, 123_456_000).unwrap(),
                    amount.to_owned(),
                ),
            )
            .await
            .unwrap();
    }

    let stored: Vec<Temporal> = db
        .select(native_temporal)
        .order_by(native_temporal.id.asc())
        .await
        .unwrap();
    assert_eq!(
        stored
            .iter()
            .map(|row| row.amount.as_str())
            .collect::<Vec<_>>(),
        ["0.00", "1.23", "-1234567890.99"]
    );
    assert_eq!(
        stored[0].day,
        chrono::NaiveDate::from_ymd_opt(2024, 5, 17).unwrap()
    );
    assert_eq!(
        stored[0].clock,
        chrono::NaiveTime::from_hms_micro_opt(23, 45, 6, 789_000).unwrap()
    );
    assert_eq!(
        stored[0].stamp,
        chrono::NaiveDate::from_ymd_opt(2024, 5, 17)
            .unwrap()
            .and_hms_micro_opt(13, 45, 6, 123_456)
            .unwrap()
    );
    assert_eq!(
        stored[0].stamp_tz,
        chrono::DateTime::from_timestamp(1_700_000_000, 123_456_000).unwrap()
    );

    // A scaled `DECIMAL` truncates toward zero into a 128-bit integer, exactly as the sqlx
    // driver's `BigDecimal` path does.
    let truncated: Vec<(u128,)> = db
        .execute(Fragment::new(
            "SELECT `amount` FROM `native_temporal` WHERE `id` = 2".to_owned(),
        ))
        .all()
        .await
        .unwrap();
    assert_eq!(truncated, [(1,)]);
}

// ---------------------------------------------------------------------------------------
// MySQL's own types: SET and BIT
// ---------------------------------------------------------------------------------------

/// A `SET('read','write','admin')` column type.
///
/// MySQL has no named `SET` type, so the labels belong to the column. A `SqlType` written by
/// hand is the declaration: its `DEF` carries the label list and its Rust type is the
/// comma-separated value MySQL itself sends.
struct Permissions;

impl SqlType for Permissions {
    const DEF: TypeDef = TypeDef::Set(EnumLabels::from_static(&["read", "write", "admin"]));
    type Rust = String;
}

impl IntoSqlValue<Permissions> for String {
    fn into_sql_value(self) -> Value {
        Value::Text(self)
    }
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "native_accounts")]
pub struct Account {
    #[besu(primary_key)]
    pub id: i64,
    #[besu(sql_type = "Permissions")]
    pub permissions: String,
    #[besu(sql_type = "besu::Bit<8>")]
    pub flags: BitString,
}

#[tokio::test]
async fn a_set_and_a_bit_round_trip() {
    let db = fresh("besu_native_set_and_bit").await;
    db.push([native_accounts.def()]).await.unwrap();
    assert!(db.push([native_accounts.def()]).await.unwrap().is_empty());

    let original = Account {
        id: 1,
        permissions: "read,admin".into(),
        flags: BitString::new("10110011").unwrap(),
    };
    db.insert(native_accounts)
        .value(native_accounts.insert(
            original.id,
            original.permissions.clone(),
            original.flags.clone(),
        ))
        .await
        .unwrap();

    let stored: Vec<Account> = db.select(native_accounts).await.unwrap();
    assert_eq!(stored, [original]);

    // A value with leading zero bits is read back as its significant bits, because MySQL
    // keeps the value as an integer and the width is not on the wire.
    db.insert(native_accounts)
        .value(native_accounts.insert(2, "read".to_owned(), BitString::new("00000101").unwrap()))
        .await
        .unwrap();
    let stored: Vec<Account> = db
        .select(native_accounts)
        .filter(native_accounts.id.eq(2))
        .await
        .unwrap();
    assert_eq!(stored[0].flags.as_str(), "101");

    let introspected = db.introspect().await.unwrap();
    let table = introspected.table("native_accounts").unwrap();
    assert_eq!(
        table.column("permissions").unwrap().ty,
        TypeDef::Set(EnumLabels::from_static(&["read", "write", "admin"]))
    );
    assert_eq!(table.column("flags").unwrap().ty, TypeDef::Bit(8));
}

// ---------------------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, besu::SqlEnum)]
#[besu(rename_all = "snake_case")]
pub enum Mood {
    Happy,
    #[besu(rename = "meh")]
    Neutral,
    Sad,
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "native_reviews")]
pub struct Review {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    #[besu(sql_type = "Mood")]
    pub mood: Mood,
}

#[tokio::test]
async fn an_enum_label_round_trips_through_decode_text() {
    let db = fresh("besu_native_enum").await;

    let first = db.push([native_reviews.def()]).await.unwrap();
    assert!(
        first
            .statements
            .iter()
            .any(|statement| statement.contains("ENUM('happy','meh','sad')")),
        "the labels must be inline: {:?}",
        first.statements
    );

    db.insert(native_reviews)
        .value(native_reviews.insert(Mood::Neutral))
        .await
        .unwrap();

    let stored: Vec<Review> = db
        .select(native_reviews)
        .filter(native_reviews.mood.eq(Mood::Neutral))
        .await
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].mood, Mood::Neutral);
    assert!(
        db.select(native_reviews)
            .filter(native_reviews.mood.eq(Mood::Sad))
            .await
            .unwrap()
            .is_empty()
    );

    let introspected = db.introspect().await.unwrap();
    assert_eq!(
        introspected
            .table("native_reviews")
            .unwrap()
            .column("mood")
            .unwrap()
            .ty,
        TypeDef::Enum(EnumDef::owned("", ["happy", "meh", "sad"]))
    );
    assert!(db.push([native_reviews.def()]).await.unwrap().is_empty());
}

// ---------------------------------------------------------------------------------------
// Spatial values, ranges and vectors
// ---------------------------------------------------------------------------------------

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "native_places")]
pub struct Place {
    #[besu(primary_key)]
    pub id: i64,
    pub any: Geometry,
    #[besu(sql_type = "besu::Point")]
    pub point: Geometry,
    #[besu(sql_type = "besu::Point<4326>")]
    pub located: Geometry,
    #[besu(sql_type = "besu::LineString<4326>")]
    pub line: Geometry,
    #[besu(sql_type = "besu::Polygon")]
    pub area: Geometry,
    #[besu(sql_type = "besu::GeometryCollection<4326>")]
    pub collection: Geometry,
    // MySQL has no range type: the canonical text is stored in a `TEXT` column.
    pub window: Range<i32>,
    // MySQL 8.4 has no vector type either: the text is stored in a `TEXT` column.
    #[besu(sql_type = "besu::VectorOf<3>")]
    pub embedding: Vector,
}

fn point(srid: bool) -> Geometry {
    if srid {
        Geometry::from_hex("0101000020E6100000000000000000F03F0000000000000040").unwrap()
    } else {
        Geometry::from_hex("0101000000000000000000F03F0000000000000040").unwrap()
    }
}

fn line(srid: bool) -> Geometry {
    let mut hex = String::from("010200");
    if srid {
        hex.push_str("0020E6100000");
    } else {
        hex.push_str("0000");
    }
    hex.push_str("02000000");
    hex.push_str("00000000000000000000000000000000");
    hex.push_str("000000000000F03F000000000000F03F");
    Geometry::from_hex(&hex).unwrap()
}

fn polygon() -> Geometry {
    Geometry::from_hex(concat!(
        "01030000000100000004000000",
        "00000000000000000000000000000000",
        "000000000000F03F0000000000000000",
        "000000000000F03F000000000000F03F",
        "00000000000000000000000000000000",
    ))
    .unwrap()
}

fn collection(srid: bool) -> Geometry {
    let mut hex = String::from("010700");
    if srid {
        hex.push_str("0020E6100000");
    } else {
        hex.push_str("0000");
    }
    hex.push_str("01000000");
    // One child point, as plain WKB.
    hex.push_str("0101000000000000000000F03F0000000000000040");
    Geometry::from_hex(&hex).unwrap()
}

#[tokio::test]
async fn spatial_types_ranges_and_vectors_round_trip() {
    let db = fresh("besu_native_spatial").await;
    db.push([native_places.def()]).await.unwrap();
    assert!(db.push([native_places.def()]).await.unwrap().is_empty());

    let original = Place {
        id: 1,
        any: point(true),
        point: point(false),
        located: point(true),
        line: line(true),
        area: polygon(),
        collection: collection(true),
        window: Range::closed(1, 5),
        embedding: Vector::new(vec![1.0, 2.5, -3.0]),
    };
    db.insert(native_places)
        .value(native_places.insert(
            original.id,
            original.any.clone(),
            original.point.clone(),
            original.located.clone(),
            original.line.clone(),
            original.area.clone(),
            original.collection.clone(),
            original.window,
            original.embedding.clone(),
        ))
        .await
        .unwrap();

    let stored: Vec<Place> = db.select(native_places).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0], original);
    assert_eq!(stored[0].located.srid(), Some(4326));
    assert_eq!(stored[0].area.kind(), Some(GeometryKind::Polygon));
    // MySQL has no range semantics, so an inclusive bound stays inclusive.
    assert_eq!(stored[0].window, Range::closed(1, 5));

    let introspected = db.introspect().await.unwrap();
    let table = introspected.table("native_places").unwrap();
    assert_eq!(
        table.column("located").unwrap().ty,
        TypeDef::Geometry {
            kind: GeometryKind::Point,
            srid: Some(4326),
        }
    );
    // A range and a vector are both plain `TEXT` on MySQL.
    assert_eq!(table.column("window").unwrap().ty, TypeDef::Text);
    assert_eq!(table.column("embedding").unwrap().ty, TypeDef::Text);
    assert!(db.push([native_places.def()]).await.unwrap().is_empty());
}

// ---------------------------------------------------------------------------------------
// Transactions and streaming
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn transactions_commit_roll_back_and_nest_savepoints() {
    let db = fresh("besu_native_transactions").await;
    db.push([native_users.def()]).await.unwrap();

    let mut tx = db.begin().await.unwrap();
    tx.insert(native_users)
        .value(native_users.insert("commit@example.com", "Commit", 1))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    tx.insert(native_users)
        .value(native_users.insert("rollback@example.com", "Rollback", 2))
        .await
        .unwrap();
    tx.rollback().await.unwrap();

    // A transaction dropped without committing must not leak into the pool, and its row must
    // not be visible.
    let mut dropped = db.begin().await.unwrap();
    dropped
        .insert(native_users)
        .value(native_users.insert("dropped@example.com", "Dropped", 3))
        .await
        .unwrap();
    drop(dropped);

    db.transaction(async |tx| {
        tx.insert(native_users)
            .value(native_users.insert("outer@example.com", "Outer", 4))
            .await?;
        // The savepoint's row is rolled back, the outer row is not.
        let inner = tx
            .transaction(async |tx| {
                tx.insert(native_users)
                    .value(native_users.insert("inner@example.com", "Inner", 5))
                    .await?;
                Err::<(), _>(<Native as besu::Driver>::error(besu::Error::invalid_query(
                    "roll back the savepoint",
                )))
            })
            .await;
        assert!(inner.is_err());
        Ok(())
    })
    .await
    .unwrap();

    let emails: Vec<String> = db
        .select(native_users)
        .pick(native_users.email)
        .order_by(native_users.id.asc())
        .await
        .unwrap();
    assert_eq!(
        emails,
        [
            "commit@example.com".to_owned(),
            "outer@example.com".to_owned()
        ]
    );
}

#[tokio::test]
async fn a_stream_yields_rows_from_a_pool_and_from_a_transaction() {
    let db = fresh("besu_native_stream").await;
    db.push([native_users.def()]).await.unwrap();
    for index in 0..5 {
        db.insert(native_users)
            .value(native_users.insert(
                format!("{index}@example.com"),
                format!("User {index}"),
                index,
            ))
            .await
            .unwrap();
    }

    let sql = db
        .select(native_users)
        .order_by(native_users.id.asc())
        .to_sql()
        .unwrap();
    let mut rows = db.stream::<User>(&sql).await.unwrap();
    let mut seen = 0;
    while let Some(user) = rows.next().await {
        let user = user.unwrap();
        assert_eq!(user.age, seen);
        seen += 1;
    }
    assert_eq!(seen, 5);

    let sql = db
        .select(native_users)
        .filter(native_users.age.gt(2))
        .order_by(native_users.id.asc())
        .to_sql()
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let mut rows = tx.stream::<User>(&sql).await.unwrap();
    let mut names = Vec::new();
    while let Some(user) = rows.next().await {
        names.push(user.unwrap().name);
    }
    drop(rows);
    tx.commit().await.unwrap();
    assert_eq!(names, ["User 3".to_owned(), "User 4".to_owned()]);
}

// ---------------------------------------------------------------------------------------
// Schema: introspection, push and migrations
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn introspection_reads_back_what_push_created() {
    let db = fresh("besu_native_introspect").await;
    db.push([native_users.def(), native_posts.def()])
        .await
        .unwrap();

    let introspected = db.introspect().await.unwrap();
    let table = introspected.table("native_users").unwrap();
    assert_eq!(table.column("id").unwrap().ty, TypeDef::Int8);
    assert!(table.column("id").unwrap().auto_increment);
    assert_eq!(table.column("email").unwrap().ty, TypeDef::Text);
    assert_eq!(table.column("age").unwrap().ty, TypeDef::Int4);
    assert!(table.column("bio").unwrap().nullable);

    let posts = introspected.table("native_posts").unwrap();
    assert_eq!(posts.foreign_keys.len(), 1);
    assert_eq!(
        posts.foreign_keys[0].references_table,
        "native_users".to_owned()
    );

    // The declared and introspected schemas agree, so a push is a no-op.
    let again = db
        .push([native_users.def(), native_posts.def()])
        .await
        .unwrap();
    assert!(again.is_empty(), "got {:?}", again.changes);
    assert!(db.push_schema(&introspected).await.unwrap().is_empty());
}

#[allow(unused_imports)]
mod account_v1 {
    use super::*;

    #[derive(Table, Debug, Clone, PartialEq)]
    #[besu(table = "native_accounts_alter")]
    pub struct AccountV1 {
        #[besu(primary_key)]
        pub id: i64,
        pub name: String,
    }
}

#[allow(unused_imports)]
mod account_v2 {
    use super::*;

    #[derive(Table, Debug, Clone, PartialEq)]
    #[besu(table = "native_accounts_alter")]
    pub struct AccountV2 {
        #[besu(primary_key)]
        pub id: i64,
        pub name: String,
        pub email: Option<String>,
    }
}

#[tokio::test]
async fn push_creates_and_alters_a_table() {
    let db = fresh("besu_native_push").await;

    let created = db
        .push([account_v1::native_accounts_alter.def()])
        .await
        .unwrap();
    assert!(
        created
            .statements
            .iter()
            .any(|statement| statement.starts_with("CREATE TABLE")),
        "got {:?}",
        created.statements
    );
    db.insert(account_v1::native_accounts_alter)
        .value(account_v1::native_accounts_alter.insert(1, "Oscar"))
        .await
        .unwrap();

    let altered = db
        .push([account_v2::native_accounts_alter.def()])
        .await
        .unwrap();
    assert!(
        altered
            .statements
            .iter()
            .any(|statement| statement.contains("ADD COLUMN")),
        "got {:?}",
        altered.statements
    );
    assert!(
        db.push([account_v2::native_accounts_alter.def()])
            .await
            .unwrap()
            .is_empty()
    );

    // The existing row survived, with the new column NULL.
    let rows: Vec<account_v2::AccountV2> =
        db.select(account_v2::native_accounts_alter).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "Oscar");
    assert_eq!(rows[0].email, None);
}

mod generated {
    #![allow(unused_imports)]
    use besu::Table;

    /// A generated column whose expression the database computes.
    #[derive(Table, Debug, Clone, PartialEq)]
    #[besu(table = "native_line_items")]
    #[allow(dead_code)]
    pub struct LineItem {
        #[besu(primary_key)]
        pub id: i64,
        pub price: i64,
        pub quantity: i64,
        #[besu(generated = "price * quantity")]
        pub total: i64,
    }
}

#[tokio::test]
async fn generated_columns_round_trip() {
    let db = fresh("besu_native_generated").await;

    let first = db.push([generated::native_line_items.def()]).await.unwrap();
    assert!(
        first.statements[0]
            .contains("`total` BIGINT GENERATED ALWAYS AS (price * quantity) STORED NOT NULL"),
        "got {:?}",
        first.statements
    );

    let introspected = db.introspect().await.unwrap();
    let total = introspected
        .table("native_line_items")
        .unwrap()
        .column("total")
        .unwrap();
    assert!(total.generated.is_some(), "got {total:?}");
    assert!(
        db.push_schema(&introspected).await.unwrap().is_empty(),
        "a generated column must not report drift"
    );

    // The database computes the value, so the column is never bound by an insert.
    db.execute(sql!(
        "INSERT INTO `native_line_items` (`id`, `price`, `quantity`) VALUES (?, ?, ?)",
        1_i64,
        5_i64,
        3_i64
    ))
    .await
    .unwrap();
    let rows: Vec<(i64,)> = db
        .execute(sql!("SELECT `total` FROM `native_line_items`"))
        .all()
        .await
        .unwrap();
    assert_eq!(rows, [(15,)]);
}

#[tokio::test]
async fn a_functional_index_and_a_check_round_trip() {
    let db = fresh("besu_native_schema_details").await;

    db.execute(sql!(
        "CREATE TABLE `events` (\
             `id` BIGINT PRIMARY KEY, \
             `kind` VARCHAR(50) NOT NULL, \
             `payload` VARCHAR(200) NOT NULL, \
             CONSTRAINT `events_kind_check` CHECK (`kind` <> 'x'))"
    ))
    .await
    .unwrap();
    db.execute(sql!("CREATE INDEX `events_kind_idx` ON `events` (`kind`)"))
        .await
        .unwrap();
    db.execute(sql!(
        "CREATE INDEX `events_lower_payload_idx` ON `events` ((lower(`payload`)))"
    ))
    .await
    .unwrap();

    let introspected = db.introspect().await.unwrap();
    let events = introspected.table("events").unwrap();
    assert_eq!(events.indexes.len(), 2, "got {:?}", events.indexes);
    assert_eq!(events.checks.len(), 1, "got {:?}", events.checks);
    assert!(
        db.push_schema(&introspected).await.unwrap().is_empty(),
        "introspection must not report drift"
    );
}

#[tokio::test]
async fn migrate_from_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("0001_create_people.sql"),
        "CREATE TABLE `native_people` (`id` BIGINT PRIMARY KEY, `name` TEXT NOT NULL);",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("0002_add_email.sql"),
        "ALTER TABLE `native_people` ADD COLUMN `email` VARCHAR(255);\n\
         CREATE UNIQUE INDEX `native_people_email_idx` ON `native_people` (`email`);",
    )
    .unwrap();

    let migrator = Migrator::from_dir(dir.path()).unwrap();
    let db = fresh("besu_native_migrate").await;

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
        "INSERT INTO `native_people` (`id`, `name`, `email`) VALUES (?, ?, ?)",
        1_i64,
        "Oscar",
        "o@example.com"
    ))
    .await
    .unwrap();
    let rows = db
        .execute(sql!(
            "SELECT `name` FROM `native_people` WHERE `email` = ?",
            "o@example.com"
        ))
        .all::<(String,)>()
        .await
        .unwrap();
    assert_eq!(rows, [("Oscar".to_owned(),)]);
}

// ---------------------------------------------------------------------------------------
// Decode and encode failures are errors, not nonsense
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_value_that_does_not_match_its_rust_type_is_an_error() {
    let db = fresh("besu_native_decode_error").await;

    let error = db
        .execute(sql!("SELECT 'not a number'"))
        .all::<i32>()
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            NativeError::Core(besu::Error::Decode { column: 0, .. })
        ),
        "unexpected error: {error}"
    );

    let mut statement = besu::Sql::<MySql>::raw("SELECT ?");
    statement.push_arg(Value::Array {
        element: TypeDef::Int4,
        values: vec![Value::Int(1)],
    });
    let error = db.driver().query(&statement).await.unwrap_err();
    assert!(
        error.to_string().contains("has no array type"),
        "unexpected error: {error}"
    );
}

/// The `Change` type is part of what `push` reports; this keeps the import honest and checks
/// that a schema change is described as one `ALTER COLUMN`.
#[tokio::test]
async fn a_changed_enum_is_one_alter_column() {
    let db = fresh("besu_native_enum_alter").await;
    db.push([native_reviews.def()]).await.unwrap();

    let mut desired = besu::schema::Schema::from_defs(&[native_reviews.def()]);
    let mood = desired.tables[0]
        .columns
        .iter_mut()
        .find(|column| column.name == "mood")
        .expect("the declared table has a mood column");
    mood.ty = TypeDef::Enum(EnumDef::owned("mood", ["happy", "meh", "ecstatic", "sad"]));

    let changes = db.push_schema(&desired).await.unwrap();
    assert_eq!(changes.changes.len(), 1, "got {:?}", changes.changes);
    assert!(
        matches!(&changes.changes[0], Change::AlterColumn { table, .. } if table == "native_reviews"),
        "got {:?}",
        changes.changes
    );
}
