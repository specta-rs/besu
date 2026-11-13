//! Driver benchmarks for `besu-mysql`.
//!
//! The `encode` group times turning a statement's parameters into `sqlx` arguments. It needs
//! nothing but the crate, so it always runs.
//!
//! The `query` group needs a real MySQL server. Set `BESU_MYSQL_BENCH_URL` to a URL whose
//! database already exists — for example `mysql://root:besu@localhost:53306/besu` — and it
//! runs against that database, dropping and recreating its own two tables. Leave it unset and
//! the group is skipped, so `cargo bench -p besu-mysql` works on a machine with nothing set
//! up.
#![cfg(feature = "sqlx")]
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use std::hint::black_box;

use besu::prelude::*;
use besu_core::{Sql, Value};
use besu_mysql::{MySql, Sqlx};
use criterion::{Criterion, criterion_group, criterion_main};

/// A row wide enough to cover the interesting decode paths: two `String`s to allocate, a
/// nullable `String` to take the `Option<T>` path, an integer, and a float.
#[derive(Table, Debug, Clone)]
#[besu(table = "bench_rows")]
pub struct BenchRow {
    #[besu(primary_key)]
    id: i64,
    name: String,
    email: String,
    age: i32,
    score: f64,
    bio: Option<String>,
}

/// A row built from the two 128-bit types, which MySQL stores as `DECIMAL(39,0)` and decodes
/// through `BigDecimal`.
#[derive(Table, Debug, Clone)]
#[besu(table = "bench_numbers")]
pub struct BenchNumber {
    #[besu(primary_key)]
    id: i64,
    value: u128,
    signed: i128,
}

/// The parameters a representative statement binds: a mix of every cost class, including the
/// two that allocate while encoding (128-bit decimal text, and JSON validated as text).
fn mixed_statement() -> Sql<MySql> {
    let mut sql = Sql::<MySql>::raw("SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?, ?");
    // `push_arg`, not `bind`: the placeholders are already there. See the PostgreSQL bench.
    sql.push_arg(Value::from(42_i64));
    sql.push_arg(Value::from("a piece of text"));
    sql.push_arg(Value::from(vec![1u8, 2, 3, 255]));
    sql.push_arg(Value::from(uuid::Uuid::nil()));
    sql.push_arg(Value::Json(br#"{"a":1,"b":[true,null]}"#.to_vec()));
    sql.push_arg(Value::UInt128(u128::MAX));
    sql.push_arg(Value::from(true));
    sql.push_arg(Value::from(2.5_f64));
    sql.push_arg(Value::from(
        chrono::NaiveDate::from_ymd_opt(2024, 5, 17).unwrap(),
    ));
    sql.push_arg(Value::Null);
    sql
}

fn bench_encode(c: &mut Criterion) {
    let mixed = mixed_statement();

    let mut group = c.benchmark_group("mysql_encode");
    group.bench_function("mixed_10", |b| {
        b.iter(|| {
            let arguments = besu_mysql::arguments_for_bench(black_box(&mixed)).unwrap();
            black_box(arguments);
        });
    });

    let insert = {
        let mut sql = Sql::<MySql>::raw("INSERT INTO bench_rows VALUES (?, ?, ?, ?, ?, ?)");
        sql.bind(1_i64);
        sql.bind("a name");
        sql.bind("someone@example.com");
        sql.bind(30_i32);
        sql.bind(1.5_f64);
        sql.bind("a biography");
        sql
    };
    group.bench_function("insert_6", |b| {
        b.iter(|| {
            let arguments = besu_mysql::arguments_for_bench(black_box(&insert)).unwrap();
            black_box(arguments);
        });
    });

    // The 128-bit bind on its own, since it is the only parameter whose encoding builds an
    // intermediate value today.
    let decimal = {
        let mut sql = Sql::<MySql>::raw("SELECT ?, ?");
        sql.bind_value(Value::UInt128(u128::MAX));
        sql.bind_value(Value::Int128(i128::MIN));
        sql
    };
    group.bench_function("decimal_2", |b| {
        b.iter(|| {
            let arguments = besu_mysql::arguments_for_bench(black_box(&decimal)).unwrap();
            black_box(arguments);
        });
    });

    group.finish();
}

/// A DDL statement with no bound values.
fn ddl(sql: &'static str) -> Fragment {
    Fragment::with_args(sql, Vec::new())
}

fn bench_query(c: &mut Criterion) {
    let Ok(url) = std::env::var("BESU_MYSQL_BENCH_URL") else {
        println!("set BESU_MYSQL_BENCH_URL to run the database benchmarks");
        return;
    };

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let db = Database::new(
        runtime
            .block_on(Sqlx::new(&url))
            .expect("the benchmark database must open"),
    );

    // Start from a known state: the tables are reused between runs so that setup is not part
    // of the measurement, but the row count has to be the same every time.
    //
    // The tables are created by hand rather than with `push`, because the database named by
    // the URL is shared with whatever else lives in it and `push` would want to drop the
    // tables it does not know about.
    runtime.block_on(async {
        for sql in [
            "DROP TABLE IF EXISTS `bench_rows`",
            "DROP TABLE IF EXISTS `bench_numbers`",
            "CREATE TABLE `bench_rows` (\
                 `id` BIGINT NOT NULL PRIMARY KEY, \
                 `name` TEXT NOT NULL, \
                 `email` TEXT NOT NULL, \
                 `age` INT NOT NULL, \
                 `score` DOUBLE NOT NULL, \
                 `bio` TEXT NULL\
             )",
            "CREATE TABLE `bench_numbers` (\
                 `id` BIGINT NOT NULL PRIMARY KEY, \
                 `value` DECIMAL(39,0) NOT NULL, \
                 `signed` DECIMAL(39,0) NOT NULL\
             )",
        ] {
            db.execute(ddl(sql)).await.unwrap();
        }

        let mut tx = db.begin().await.unwrap();
        for id in 0..1_000_i64 {
            tx.insert(bench_rows)
                .value(
                    bench_rows
                        .insert(
                            id,
                            format!("Name {id}"),
                            format!("user{id}@example.com"),
                            20 + (id % 50) as i32,
                            id as f64 / 3.0,
                        )
                        .bio(None::<String>),
                )
                .await
                .unwrap();
        }
        for id in 0..1_000_i64 {
            tx.insert(bench_numbers)
                .value(bench_numbers.insert(id, u128::MAX - id as u128, -id as i128))
                .await
                .unwrap();
        }
        tx.commit().await.unwrap();
    });

    let mut group = c.benchmark_group("mysql_query");
    group.bench_function("select_1000_rows", |b| {
        b.iter(|| {
            let rows: Vec<BenchRow> = runtime
                .block_on(async { db.select(bench_rows).order_by(bench_rows.id.asc()).await })
                .unwrap();
            black_box(rows);
        });
    });

    group.bench_function("select_1000_decimals", |b| {
        b.iter(|| {
            let rows: Vec<BenchNumber> = runtime
                .block_on(async {
                    db.select(bench_numbers)
                        .order_by(bench_numbers.id.asc())
                        .await
                })
                .unwrap();
            black_box(rows);
        });
    });

    let mut next = 1_000_i64;
    group.bench_function("insert_one_row", |b| {
        b.iter(|| {
            let id = next;
            next += 1;
            runtime
                .block_on(async {
                    db.insert(bench_rows)
                        .value(
                            bench_rows
                                .insert(
                                    id,
                                    format!("Name {id}"),
                                    format!("user{id}@example.com"),
                                    30,
                                    2.5,
                                )
                                .bio(Some("a biography".to_string())),
                        )
                        .await
                })
                .unwrap();
        });
    });
    group.finish();

    runtime.block_on(async {
        db.execute(ddl("DROP TABLE IF EXISTS `bench_rows`"))
            .await
            .unwrap();
        db.execute(ddl("DROP TABLE IF EXISTS `bench_numbers`"))
            .await
            .unwrap();
    });
}

criterion_group!(benches, bench_encode, bench_query);
criterion_main!(benches);
