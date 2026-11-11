//! Driver benchmarks for `besu-sqlite`.
//!
//! The `encode` group times encoding a statement's parameters for `sqlx`. It needs nothing
//! but the crate, so it always runs.
//!
//! The `query` group needs a real SQLite file. Set `BESU_SQLITE_BENCH_URL` to a path — for
//! example `/tmp/besu-bench.db` — and it runs against that file; leave it unset and the group
//! is skipped, so `cargo bench -p besu-sqlite` works on a machine with nothing set up.
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use std::{hint::black_box, path::PathBuf};

use besu::prelude::*;
use besu_core::{Sql, Value};
use besu_sqlite::{SQLite, Sqlx};
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

/// The parameters a representative insert binds: a mix of every cost class, including the
/// two that allocate while encoding (128-bit decimal text, and JSON serialised to text).
fn mixed_statement() -> Sql<SQLite> {
    // The placeholders are already in the text, so `push_arg` keeps this readable; what is
    // being measured is the encoding of ten values, not the spelling of the statement.
    let mut sql = Sql::<SQLite>::raw("SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?, ?");
    sql.push_arg(Value::Int(42));
    sql.push_arg(Value::Text("a piece of text".into()));
    sql.push_arg(Value::Bytes(vec![1, 2, 3, 255]));
    sql.push_arg(Value::Uuid(uuid::Uuid::nil()));
    sql.push_arg(Value::Json(br#"{"a":1,"b":[true,null]}"#.to_vec()));
    sql.push_arg(Value::UInt128(u128::MAX));
    sql.push_arg(Value::Bool(true));
    sql.push_arg(Value::Float(2.5));
    sql.push_arg(Value::Date(
        chrono::NaiveDate::from_ymd_opt(2024, 5, 17).unwrap(),
    ));
    sql.push_arg(Value::Null);
    sql
}

fn bench_encode(c: &mut Criterion) {
    let mixed = mixed_statement();

    let mut group = c.benchmark_group("sqlite_encode");
    group.bench_function("sqlite_mixed_10", |b| {
        b.iter(|| {
            let arguments = besu_sqlite::arguments_for_bench(black_box(&mixed)).unwrap();
            black_box(arguments);
        });
    });

    let insert = {
        let mut sql = Sql::<SQLite>::raw("INSERT INTO bench_rows VALUES (?, ?, ?, ?, ?, ?)");
        sql.push_arg(Value::Int(1));
        sql.push_arg(Value::Text("a name".into()));
        sql.push_arg(Value::Text("someone@example.com".into()));
        sql.push_arg(Value::Int(30));
        sql.push_arg(Value::Float(1.5));
        sql.push_arg(Value::Text("a biography".into()));
        sql
    };
    group.bench_function("sqlite_insert_6", |b| {
        b.iter(|| {
            let arguments = besu_sqlite::arguments_for_bench(black_box(&insert)).unwrap();
            black_box(arguments);
        });
    });
    group.finish();
}

fn bench_query(c: &mut Criterion) {
    let Some(path) = std::env::var_os("BESU_SQLITE_BENCH_URL").map(PathBuf::from) else {
        println!("set BESU_SQLITE_BENCH_URL to run the database benchmarks");
        return;
    };

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let db = besu::Database::new(
        runtime
            .block_on(Sqlx::new(&path))
            .expect("the benchmark database must open"),
    );

    // Start from a known state: the file is reused between runs so that setup is not part of
    // the measurement, but the row count has to be the same every time.
    runtime.block_on(async {
        db.execute(sql!("DROP TABLE IF EXISTS \"bench_rows\""))
            .await
            .unwrap();
        db.push([bench_rows.def()]).await.unwrap();

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
        tx.commit().await.unwrap();
    });

    let mut group = c.benchmark_group("sqlite_query");
    group.bench_function("sqlite_select_1000_rows", |b| {
        b.iter(|| {
            let rows: Vec<BenchRow> = runtime
                .block_on(async { db.select(bench_rows).order_by(bench_rows.id.asc()).await })
                .unwrap();
            black_box(rows);
        });
    });

    let mut next = 1_000_i64;
    group.bench_function("sqlite_insert_one_row", |b| {
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
        db.execute(sql!("DROP TABLE IF EXISTS \"bench_rows\""))
            .await
            .unwrap();
    });
}

criterion_group!(benches, bench_encode, bench_query);
criterion_main!(benches);
